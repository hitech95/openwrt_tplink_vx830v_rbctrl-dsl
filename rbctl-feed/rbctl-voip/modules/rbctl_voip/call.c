/* SPDX-License-Identifier: GPL-2.0-or-later */
#include <errno.h>
#include <string.h>
#include <time.h>
#include <re.h>
#include <baresip.h>
#include "phone.h"

/* One configured FXS line, one account, one call. All entries on libre's loop. */
static struct {
	unsigned line;
	struct call *call; /* borrowed until CLOSED */
	struct ua *ua;
	bool offhook, collecting, ringing, open, media;
	char digits[33];
	size_t n;
} p;

unsigned phone_slot(void) { return p.line; }

static int words(unsigned cmd)
{
	return phone_command((rbctl_voip_record){.command=cmd, .body.words={p.line}});
}

static int tone(unsigned selector)
{
	return phone_command((rbctl_voip_record){.command=13, .body.tone.args={p.line,selector}});
}

static int open_slot(void)
{
	if (p.open) return 0;
	int err = phone_command((rbctl_voip_record){.command=8,
		.body.words={p.line,0,0,1,0,0,0,0,0,1,0}});
	if (!err) p.open = true;
	return err;
}

static int media_on(void)
{
	if (p.media) return 0;
	int err = open_slot();
	if (!err) err = phone_command((rbctl_voip_record){.command=10, .body.media={
		.activation=0xfffe, .slot=p.line, .direction=3, .codec=2, .tx_pt=8, .rx_pt=8,
		.tx_ms=20, .rx_ms=20, .dtmf=1, .tx_event_pt=101, .rx_event_pt=101,
		.local4={127,0,0,1}, .remote4={127,0,0,1}, .family=2,
		.local_port=4000+2*p.line, .remote_port=5000+2*p.line, .control_port=5001+2*p.line}});
	if (!err) p.media = true;
	return err;
}

static void release(void)
{
	phone_audio_mode(false,false);
	if (phone_board_ready()) {
		int err = 0;
		if (p.ringing) err |= words(6);
		if (p.media) err |= words(12);
		if (p.open) { err |= tone(0); err |= words(31); }
		if (err) warning("rbctl_voip: slot cleanup not confirmed\n");
	}
	p.open = p.media = p.ringing = p.collecting = false;
	p.n = 0;
	p.call = NULL;
	p.ua = NULL;
}

static void hangup(void)
{
	struct ua *ua = p.ua;
	struct call *call = p.call;
	release();
	if (call) ua_hangup(ua,call,0,NULL);
}

static int dial(void)
{
	struct le *le = list_head(uag_list());
	if (!le || !p.n) return ENOENT;
	p.ua = le->data;
	struct pl ext;
	char *uri = NULL;
	p.digits[p.n] = '\0';
	pl_set_str(&ext,p.digits);
	int err = account_uri_complete_strdup(ua_account(p.ua),&uri,&ext);
	p.collecting = false;
	if (!err) err = ua_connect(p.ua,&p.call,NULL,uri,VIDMODE_OFF);
	mem_deref(uri);
	return err;
}

void phone_hook(const rbctl_voip_record *r)
{
	if (r->body.words[0] != p.line) return;
	int err = 0;
	if (r->command == 17) { p.offhook = false; hangup(); return; }
	if (r->command == 18 && !p.offhook) {
		p.offhook = true;
		if (p.ringing && p.call) {
			err = words(6);
			if (!err) { p.ringing = false; err = ua_answer(p.ua,p.call,VIDMODE_OFF); }
		}
		else if (!p.call) {
			err = open_slot();
			if (!err) err = tone(4);
			p.collecting = !err;
		}
	}
	else if (r->command == 20 || r->command == 24) {
		unsigned key = r->body.words[1];
		if (!(key >= '0' && key <= '9') && key != '*' && key != '#') return;
		if (p.collecting && r->command == 20) {
			if (!p.n && key == '#') return;
			if (!p.n) err = tone(0);
			if (!err && key == '#') err = dial();
			else if (!err && p.n < sizeof(p.digits)-1) p.digits[p.n++] = (char)key;
			else if (!err) { p.n = 0; err = tone(4); }
		}
		else if (p.call && call_state(p.call) == CALL_STATE_ESTABLISHED)
			err = call_send_digit(p.call,r->command == 24 ? (char)key : KEYCODE_REL);
	}
	if (err) { warning("rbctl_voip: handset action failed (%m)\n",err); hangup(); }
}

static int ring(struct call *call)
{
	rbctl_voip_record cid = {.command=4, .body.cid={.line=p.line,.select=1u<<p.line}};
	struct pl str;
	struct uri uri;
	pl_set_str(&str, call_peeruri(call));
	if (!uri_decode(&uri,&str)) {
		for (size_t i=0; i<uri.user.l && i<15; ++i) {
			char c=uri.user.p[i];
			if ((c<'0' || c>'9') && c!='+') break;
			cid.body.cid.number[i]=(uint8_t)c;
		}
	}
	const char *name = call_peername(call);
	for (size_t i=0; name && i<15 && name[i]>=32 && name[i]<127; ++i)
		cid.body.cid.name[i]=(uint8_t)name[i];
	time_t now=time(NULL); struct tm tm;
	if (localtime_r(&now,&tm)) strftime((char *)cid.body.cid.date,9,"%m%d%H%M",&tm);
	int err = phone_command(cid);
	if (!err) err = phone_command((rbctl_voip_record){.command=5,
		.body.words={p.line,0,0,UINT32_MAX,1u<<p.line,0}});
	if (!err) p.ringing=true;
	return err;
}

static void sip_event(enum ua_event ev, struct bevent *event, void *arg)
{
	(void)arg;
	if (ev == UA_EVENT_SIPSESS_CONN) {
		const struct sip_msg *msg=bevent_get_msg(event);
		struct ua *ua=msg ? uag_find_msg(msg) : NULL;
		if (!ua) return;
		bevent_stop(event);
		if (p.call || p.offhook || !phone_board_ready())
			(void)sip_treply(NULL,uag_sip(),msg,486,"Busy Here");
		else if (ua_accept(ua,msg))
			warning("rbctl_voip: incoming session accept failed\n");
		return;
	}
	struct call *call = bevent_get_call(event);
	if (!call) return;
	int err = 0;
	if (ev == UA_EVENT_CALL_INCOMING) {
		struct ua *ua = bevent_get_ua(event);
		if (p.call || p.offhook || !phone_board_ready()) {
			ua_hangup(ua,call,486,"Busy Here"); return;
		}
		p.call=call; p.ua=ua; err=ring(call);
	}
	else if (p.call != call) return;
	else if (ev == UA_EVENT_CALL_RINGING || ev == UA_EVENT_CALL_PROGRESS || ev == UA_EVENT_CALL_ESTABLISHED) {
		err=media_on();
		if (!err) phone_audio_mode(true,ev == UA_EVENT_CALL_RINGING);
	}
	else if (ev == UA_EVENT_CALL_CLOSED) release();
	if (err) { warning("rbctl_voip: call setup failed (%m)\n",err); hangup(); }
}

void phone_board_lost(void) { hangup(); }
int phone_init(unsigned line)
{
	p.line=line;
	int err=phone_audio_init();
	if (!err) err=bevent_register(sip_event,NULL);
	if (err) phone_audio_close();
	return err;
}
void phone_close(void)
{
	bevent_unregister(sip_event);
	hangup();
	phone_audio_close();
	p.offhook=false;
}
