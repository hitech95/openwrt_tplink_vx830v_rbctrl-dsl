/* SPDX-License-Identifier: GPL-2.0-or-later */
#include <errno.h>
#include <string.h>
#include <strings.h>
#include <stdlib.h>
#include <time.h>
#include <re.h>
#include <baresip.h>
#include "phone.h"
#include "digitmap.h"
#include "manage.h"

/* A dialog never changes its FXS owner. Callbacks may reenter via ua_hangup. */
struct phone_line {
	struct digitmap map;
	struct tmr timer;
	struct tmr resume_timer;
	bool resume_pending;
	unsigned first_ms, short_ms, long_ms, warn_ms, lock_ms;
	int tx_gain, rx_gain;
	unsigned busy_sel, congestion_sel, disconnect_sel;
	bool inband, require_reg, reg_blocked, mwi, messages, cw, solo_hold;
	bool cid_enabled, cw_cid, hunt;
	bool fax_profile, fax_active;
	char alert_match[80];
	unsigned alert_entry;
	/* Identity/ring profile for the INVITE being accepted, copied before
	 * the SIP message is released. Materialize a record per physical line. */
	rbctl_voip_cid incoming_id;
	unsigned incoming_cadence;
	enum { TIME_NONE, TIME_FIRST, TIME_SHORT, TIME_LONG, TIME_WARN, TIME_LOCK } timeout;
	bool enabled;
	char aor[256];
	unsigned line;
	struct call *call; /* borrowed until CLOSED */
	struct call *other; /* unanswered waiting dialog or locally held dialog */
	bool other_held, recall;
	struct ua *ua;
	bool offhook, collecting, ringing, open, media, connected;
	char digits[33];
	size_t n;
};
static struct phone_line lines[PHONE_LINES];
static struct tmr reload_timer;
static bool reload_pending;

bool voice_reload_pending(void) { return reload_pending; }
static bool lines_busy(void)
{
	for (unsigned i=0;i<PHONE_LINES;++i)
		if (lines[i].offhook || lines[i].call || lines[i].other || lines[i].open) return true;
	return false;
}
static void apply_when_idle(void *unused)
{
	(void)unused;
	if (lines_busy()) { tmr_start(&reload_timer,250,apply_when_idle,NULL); return; }
	reload_pending=false;
	int err=voice_reload();
	if (err) warning("rbctl_voip: queued reload failed (%m)\n",err);
}

static int words(struct phone_line *p, unsigned cmd)
{
	return phone_command((rbctl_voip_record){.command=cmd, .body.words={p->line}});
}

static int tone(struct phone_line *p, unsigned selector)
{
	return phone_command((rbctl_voip_record){.command=13, .body.tone.args={p->line,selector}});
}

static int open_slot(struct phone_line *p)
{
	if (p->open) return 0;
	int err = phone_command((rbctl_voip_record){.command=8,
		.body.words={p->line,(uint32_t)p->rx_gain,(uint32_t)p->tx_gain,1,0,0,0,0,0,1,0}});
	if (!err) p->open = true;
	return err;
}

static int media_record(struct phone_line *p, unsigned command)
{
	bool relay=!p->inband && !p->fax_active;
	return phone_command((rbctl_voip_record){.command=command, .body.media={
		.activation=0xfffe, .slot=p->line, .direction=3, .codec=2, .tx_pt=8, .rx_pt=8,
		.tx_ms=20, .rx_ms=20, .dtmf=relay, .tx_event_pt=relay?101:0, .rx_event_pt=relay?101:0,
		.local4={127,0,0,1}, .remote4={127,0,0,1}, .family=2,
		.local_port=4000+2*p->line, .remote_port=5000+2*p->line, .control_port=5001+2*p->line}});
}

static int media_on(struct phone_line *p)
{
	if (p->media) return 0;
	int err=open_slot(p);
	if (!err) err=media_record(p,10);
	if (!err) p->media = true;
	return err;
}

static void release(struct phone_line *p)
{
	tmr_cancel(&p->resume_timer); p->resume_pending=false;
	tmr_cancel(&p->timer); p->timeout=TIME_NONE; p->reg_blocked=false; p->solo_hold=false; p->fax_active=false;
	phone_audio_mode(p->line,false,false);
	if (phone_board_ready()) {
		int err = 0;
		if (p->ringing) err |= words(p,6);
		if (p->media) err |= words(p,12);
		if (p->open) { err |= tone(p,0); err |= words(p,31); }
		if (err) warning("rbctl_voip: slot cleanup not confirmed\n");
	}
	p->open = p->media = p->ringing = p->collecting = p->connected = false;
	p->n = 0;
	p->call = NULL;
	p->other = NULL;
	p->other_held = p->recall = false;
	p->ua = NULL;
}

static void hangup(struct phone_line *p)
{
	struct ua *ua = p->ua;
	struct call *call = p->call;
	struct call *other = p->other;
	release(p);
	if (call) ua_hangup(ua,call,0,NULL);
	if (other) ua_hangup(ua,other,0,NULL);
}

static int ring(struct phone_line *p, struct call *call);

static int select_audio(struct phone_line *p, struct ua *ua)
{
	/* A shared account has only one device default. Select it immediately
	 * before creating/accepting the dialog on the same event-loop turn. */
	const char *dev=p->line ? "phone1" : "phone0";
	int err=account_set_ausrc_dev(ua_account(ua),dev);
	return err ? err : account_set_auplay_dev(ua_account(ua),dev);
}

static int resume(struct phone_line *p, struct call *c)
{
	const char *dev=p->line ? "phone1" : "phone0";
	int err=audio_set_devicename(call_audio(c),dev,dev);
	if (!err) err=call_hold(c,false);
	if (!err) err=audio_set_source(call_audio(c),"rbctl_voip",dev);
	if (!err) err=audio_set_player(call_audio(c),"rbctl_voip",dev);
	return err;
}

static void solo_resume(void *arg)
{
	struct phone_line *p=arg;
	if (!p->resume_pending || !p->solo_hold || !p->call || !call_refresh_allowed(p->call)) return;
	p->resume_pending=false;
	if (tone(p,0) || resume(p,p->call) || media_on(p)) { hangup(p); return; }
	p->solo_hold=false; phone_audio_mode(p->line,true,false);
}

static void flash(struct phone_line *p)
{
	if (!p->connected) return;
	phone_digit(p->line,0);
	if (!p->other) {
		if (p->solo_hold) {
			p->resume_pending=!p->resume_pending;
			solo_resume(p);
		} else {
			if (call_hold(p->call,true)) return;
			audio_set_source(call_audio(p->call),NULL,NULL);
			audio_set_player(call_audio(p->call),NULL,NULL);
			if (audio_set_devicename(call_audio(p->call),"held","held") || words(p,12)) { hangup(p); return; }
			phone_audio_mode(p->line,false,false); p->media=false; p->solo_hold=true;
			if (tone(p,4)) hangup(p);
		}
		return;
	}
	struct call *old=p->call, *next=p->other;
	bool held=p->other_held;
	int err=call_hold(old,true);
	if (err) { (void)call_hold(old,false); return; }
	/* The two dialogs share one physical audio device. Detach A before B
	 * can allocate it; a held dialog must never receive handset samples. */
	audio_set_source(call_audio(old),NULL,NULL);
	audio_set_player(call_audio(old),NULL,NULL);
	err=audio_set_devicename(call_audio(old),"held","held");
	if (err) { if (resume(p,old)) hangup(p); return; }
	p->call=next; p->other=old; p->other_held=true; p->connected=held;
	phone_wait_beep(p->line,false);
	err=held ? resume(p,next) : ua_answer(p->ua,next,VIDMODE_OFF);
	if (err) {
		warning("rbctl_voip: flash switch failed (%m)\n",err);
		/* Restore A only if synchronous close callbacks haven't done so. */
		if (p->call==next && p->other==old) {
			p->call=old; p->other=NULL; p->other_held=false; p->connected=true;
			ua_hangup(p->ua,next,0,NULL);
			if (resume(p,old)) hangup(p);
		}
	}
}

static struct ua *mapped_ua(const struct phone_line *p)
{
	if (*p->aor) return uag_find_aor(p->aor);
	struct le *le = list_head(uag_list());
	return le ? le->data : NULL;
}

static int dial(struct phone_line *p)
{
	tmr_cancel(&p->timer); p->timeout=TIME_NONE;
	p->ua = mapped_ua(p);
	if (!p->ua || !p->n) return ENOENT;
	struct pl ext;
	char *uri = NULL;
	p->digits[p->n] = '\0';
	pl_set_str(&ext,p->digits);
	int err = account_uri_complete_strdup(ua_account(p->ua),&uri,&ext);
	if (!err) err=select_audio(p,p->ua);
	p->collecting = false;
	if (!err) err = ua_connect(p->ua,&p->call,NULL,uri,VIDMODE_OFF);
	mem_deref(uri);
	return err;
}

const char *phone_panel_state(void)
{
	bool active=false, registered=false;
	if (!phone_board_ready()) return "FAULT\n";
	for (unsigned i=0;i<PHONE_LINES;++i) {
		const struct phone_line *p=&lines[i];
		if (!p->enabled) continue;
		if (p->ringing || (p->other && !p->other_held)) return "RINGING\n";
		active |= p->offhook || p->call!=NULL;
		struct ua *ua=mapped_ua(p);
		registered |= ua && ua_isregistered(ua);
	}
	return active ? "ACTIVE\n" : registered ? "READY\n" : "FAULT\n";
}

static void on_timeout(void *arg);
static void arm(struct phone_line *p, int kind, unsigned ms)
{
	p->timeout=kind; tmr_start(&p->timer,ms,on_timeout,p);
}

/* The SIP dialog has gone; retain only local resources for the indication. */
static int progress(struct phone_line *p, unsigned code)
{
	p->collecting=false;
	int err=open_slot(p);
	bool sit=code==404 || code==410 || code==484 || code==604;
	if (!err && sit) {
		err=tone(p,0);
		if (!err) err=media_on(p);
		if (!err) { phone_audio_mode(p->line,true,false); phone_local_signal(p->line,SIGNAL_UNOBTAINABLE); }
	} else if (!err) {
		phone_audio_mode(p->line,false,false);
		if (p->media) { err=words(p,12); p->media=false; }
		bool busy=code==486 || code==600 || code==603 || code==408 || code==480 || code==487;
		if (!err) err=tone(p,busy?p->busy_sel:p->congestion_sel);
	}
	if (!err) arm(p,TIME_WARN,p->warn_ms);
	return err;
}

static void on_timeout(void *arg)
{
	struct phone_line *p=arg;
	if (!p->offhook || !phone_board_ready()) return;
	int err=0;
	switch (p->timeout) {
	case TIME_SHORT: err=dial(p); if (err) err=progress(p,503); break;
	case TIME_LONG: err=progress(p,484); break;
	case TIME_FIRST: case TIME_WARN:
		p->collecting=false; err=tone(p,0);
		if (!err) err=media_on(p);
		if (!err) { phone_audio_mode(p->line,true,false); phone_local_signal(p->line,SIGNAL_HOWLER); arm(p,TIME_LOCK,p->lock_ms); }
		break;
	case TIME_LOCK: release(p); break;
	default: break;
	}
	if (err) hangup(p);
	voice_changed();
}

static int start_collecting(struct phone_line *p)
{
	p->n=0; p->digits[0]=0;
	int err=open_slot(p);
	if (!err && p->require_reg && !ua_isregistered(mapped_ua(p))) {
		p->reg_blocked=true; return progress(p,503);
	}
	if (!err) err=tone(p,p->mwi && p->messages ? 19 : 4);
	if (!err) { p->collecting=true; arm(p,TIME_FIRST,p->first_ms); }
	return err;
}

void phone_fax(unsigned slot, bool active)
{
	if (slot>=PHONE_LINES) return;
	struct phone_line *p=&lines[slot];
	if (!p->fax_profile || !p->connected || !p->media || p->fax_active==active) return;
	if (active && (conf_config()->avt.audio.jbtype!=JBUF_FIXED || conf_config()->audio.adaptive || mod_find("plc"))) {
		warning("rbctl_voip: fax profile requires fixed buffers and PLC disabled\n"); return;
	}
	const struct aucodec *codec=audio_codec(call_audio(p->call),true);
	if (active && (!codec || (strcasecmp(codec->name,"PCMA") && strcasecmp(codec->name,"PCMU")))) {
		warning("rbctl_voip: fax profile needs negotiated G.711\n"); return;
	}
	p->fax_active=active; phone_digit(slot,0); phone_wait_beep(slot,false);
	if (media_record(p,11)) { p->fax_active=!active; return; }
	info("rbctl_voip: line %u fax passthrough %s requested\n",slot,active?"on":"off");
}

bool phone_detect_fax(unsigned slot)
{
	return lines[slot].fax_profile && lines[slot].connected && !lines[slot].fax_active;
}

uint64_t phone_rx_buffer_ms(unsigned slot)
{
	struct call *c=lines[slot].call;
	return c ? audio_jb_current_value(call_audio(c)) : 0;
}

static void handle_hook(const rbctl_voip_record *r)
{
	unsigned slot = r->body.words[0];
	if (slot >= PHONE_LINES || !lines[slot].enabled) return;
	struct phone_line *p = &lines[slot];
	int err = 0;
	if (r->command==21 || r->command==22) { phone_fax(slot,r->command==21); return; }
	if (r->command == 19 && p->offhook) { flash(p); return; }
	if (r->command == 17) {
		if (!p->offhook) return;
		p->offhook = false;
		struct call *other=p->other;
		struct ua *ua=p->ua;
		bool held=p->other_held;
		p->other=NULL; hangup(p);
		if (other) {
			p->ua=ua; p->call=other; p->recall=held;
			if (ring(p,other)) hangup(p);
		}
		return;
	}
	if (r->command == 18 && !p->offhook) {
		p->offhook = true;
		if (p->ringing && p->call) {
			/* First answer wins. Other ringing appearances only own their
			 * local SLIC ring, never a second reference to hang up. */
			for (unsigned i=0;i<PHONE_LINES;++i) {
				struct phone_line *q=&lines[i];
				if (q!=p && q->ringing && q->call==p->call) release(q);
			}
			err = words(p,6);
			if (!err) {
				p->ringing = false;
				if (p->recall) {
					p->recall=false; err=resume(p,p->call);
					if (!err) err=media_on(p);
					if (!err) { p->connected=true; phone_audio_mode(p->line,true,false); }
				}
				else {
					const char *dev=p->line?"phone1":"phone0";
					err=audio_set_devicename(call_audio(p->call),dev,dev);
					if (!err) err=ua_answer(p->ua,p->call,VIDMODE_OFF);
				}
			}
		}
		else if (!p->call) {
			err = start_collecting(p);
		}
	}
	else if (r->command == 20 || r->command == 24) {
		unsigned key = r->body.words[1];
		if (!(key >= '0' && key <= '9') && key != '*' && key != '#') return;
		if (p->collecting && r->command == 20) {
			if (!p->n && key == '#') return;
			if (!p->n) err = tone(p,0);
			if (!err && key == '#') { err = dial(p); if (err) err=progress(p,503); }
			else if (!err && p->n < sizeof(p->digits)-1) {
				p->digits[p->n++]=(char)key; p->digits[p->n]=0;
				switch (digitmap_match(&p->map,p->digits)) {
				case DIAL_EXACT: err=dial(p); if (err) err=progress(p,503); break;
				case DIAL_POSSIBLE: arm(p,TIME_SHORT,p->short_ms); break;
				case DIAL_PREFIX: arm(p,TIME_LONG,p->long_ms); break;
				case DIAL_INVALID: err=progress(p,484); break;
				}
			}
			else if (!err) err=start_collecting(p);
		}
		else if (p->call && !p->solo_hold && !p->fax_active && call_state(p->call) == CALL_STATE_ESTABLISHED) {
			if (p->inband) phone_digit(p->line,r->command==24 ? (char)key : 0);
			else err = call_send_digit(p->call,r->command == 24 ? (char)key : KEYCODE_REL);
		}
	}
	if (err) { warning("rbctl_voip: handset action failed (%m)\n",err); hangup(p); }
}

void phone_hook(const rbctl_voip_record *r)
{
	handle_hook(r);
	voice_changed();
}

static void copy_name(uint8_t *dst,const struct pl *name)
{
	/* Copy complete UTF-8 code points, at most 15 bytes. */
	size_t i=0;
	while (i<name->l && i<15) {
		unsigned c=(uint8_t)name->p[i];
		size_t width=c<128?1:c>=194 && c<=223?2:c>=224 && c<=239?3:c>=240 && c<=244?4:0;
		if (!width || c<32 || c==127 || i+width>15 || i+width>name->l) break;
		for (size_t j=1;j<width;++j) if (((uint8_t)name->p[i+j]&192)!=128) return;
		memcpy(dst+i,name->p+i,width); i+=width;
	}
}

static void prepare_identity(struct phone_line *p,const struct sip_msg *msg)
{
	rbctl_voip_cid *id=&p->incoming_id; memset(id,0,sizeof(*id));
	id->line=p->line; id->select=1u<<p->line;
	struct sip_addr addr={.dname=msg->from.dname,.uri=msg->from.uri,.auri=msg->from.auri};
	const struct sip_hdr *pai=sip_msg_xhdr(msg,"P-Asserted-Identity"), *rpid=sip_msg_xhdr(msg,"Remote-Party-ID");
	struct sip_addr tmp;
	if (pai && !sip_addr_decode(&tmp,&pai->val)) addr=tmp;
	else if (rpid && !sip_addr_decode(&tmp,&rpid->val)) addr=tmp;
	bool withheld=!pl_strcasecmp(&msg->from.uri.user,"anonymous") || !pl_strcasecmp(&msg->from.uri.host,"anonymous.invalid");
	const struct sip_hdr *priv=sip_msg_xhdr(msg,"Privacy");
	if (priv) {
		/* On ambiguous privacy directives err on the side of nondisclosure. */
		withheld |= pl_strcasecmp(&priv->val,"none")!=0;
	}
	if (rpid) {
		struct pl v;
		if (!msg_param_decode(&rpid->val,"privacy",&v) && pl_strcasecmp(&v,"off")) withheld=true;
	}
	if (withheld) id->number[0]='P';
	else {
		copy_name(id->name,&addr.dname);
		struct pl user=addr.uri.user;
		if (!pl_strcasecmp(&addr.uri.scheme,"tel") && addr.auri.l>4) {user.p=addr.auri.p+4;user.l=addr.auri.l-4;}
		for (size_t i=0;i<user.l && i<15;++i) {
			char c=user.p[i]; if ((c<'0'||c>'9') && !(i==0 && c=='+')) break;
			id->number[i]=(uint8_t)c;
		}
		if (!id->number[0]) id->number[0]='O';
	}
	p->incoming_cadence=0;
	const struct sip_hdr *alert=sip_msg_hdr(msg,SIP_HDR_ALERT_INFO);
	if (alert && *p->alert_match) {
		struct pl v;
		if (!pl_strcasecmp(&alert->val,p->alert_match) ||
		    (!msg_param_decode(&alert->val,"info",&v) && !pl_strcasecmp(&v,p->alert_match)))
			p->incoming_cadence=p->alert_entry;
	}
}

static int send_cid(struct phone_line *p)
{
	rbctl_voip_record cid = {.command=4,.body.cid=p->incoming_id};
	if (!p->cid_enabled) {
		memset(&cid.body.cid, 0, sizeof(cid.body.cid));
		cid.body.cid.line = p->line;
	}
	else {
		time_t now = time(NULL);
		struct tm tm;
		if (localtime_r(&now, &tm))
			strftime((char *)cid.body.cid.date, 9, "%m%d%H%M", &tm);
	}
	return phone_command(cid);
}

static int ring(struct phone_line *p, struct call *call)
{
	(void)call;
	int err=send_cid(p);
	if (!err) err = phone_command((rbctl_voip_record){.command=5,
		.body.words={p->line,0,p->incoming_cadence,UINT32_MAX,1u<<p->line,0}});
	if (!err) p->ringing=true;
	return err;
}

static void sip_event(enum bevent_ev ev, struct bevent *event, void *arg)
{
	(void)arg;
	struct ua *ua = bevent_get_ua(event);
	if (ev == BEVENT_SIPSESS_CONN && bevent_get_msg(event)) ua=uag_find_msg(bevent_get_msg(event));
	if (!ua) return;
	struct call *call=bevent_get_call(event);
	struct phone_line *p=NULL;
	struct phone_line *free_lines[PHONE_LINES]; unsigned nf=0;
	for (unsigned i=0;i<PHONE_LINES;++i) {
		struct phone_line *q=&lines[i];
		if (!q->enabled || mapped_ua(q)!=ua) continue;
		if (!q->call && !q->offhook) free_lines[nf++]=q;
		if (!p || (q->connected && q->cw && !q->other && !q->solo_hold)) p=q;
	}
	if (!p) return;
	if ((ev==BEVENT_SIPSESS_CONN || ev==BEVENT_CALL_INCOMING) && nf) p=free_lines[0];
	if (ev==BEVENT_MWI_NOTIFY) {
		const char *s=bevent_get_text(event);
		while (s && *s) {
			size_t n=strcspn(s,"\r\n");
			if (n>=17 && !strncasecmp(s,"Messages-Waiting:",17)) {
				const char *v=s+17,*end=s+n;
				while (v<end && (*v==' ' || *v=='\t')) ++v;
				while (end>v && (end[-1]==' ' || end[-1]=='\t')) --end;
				bool yes=end-v==3 && !strncasecmp(v,"yes",3);
				if (!yes && !(end-v==2 && !strncasecmp(v,"no",2))) break;
				for (unsigned i=0;i<PHONE_LINES;++i) {
					struct phone_line *q=&lines[i];
					if (!q->enabled || !q->mwi || mapped_ua(q)!=ua) continue;
					q->messages=yes;
					if (q->collecting && !q->n) (void)tone(q,yes?19:4);
				}
				break;
			}
			s+=n; s+=strspn(s,"\r\n");
		}
		return;
	}
	if (ev==BEVENT_REGISTER_OK) {
		for (unsigned i=0;i<PHONE_LINES;++i) {
			struct phone_line *q=&lines[i];
			if (q->enabled && mapped_ua(q)==ua && q->reg_blocked && q->offhook && !q->call) {
				release(q); if (start_collecting(q)) release(q);
			}
		}
		return;
	}
	if (ev==BEVENT_REGISTER_FAIL || ev==BEVENT_UNREGISTERING) {
		for (unsigned i=0;i<PHONE_LINES;++i) {
			struct phone_line *q=&lines[i];
			if (q->enabled && q->require_reg && mapped_ua(q)==ua &&
			    !ua_isregistered(ua) && q->offhook && q->collecting && !q->call) {
				q->reg_blocked=true;
				if (progress(q,503)) release(q);
			}
		}
		return;
	}
	if (ev == BEVENT_SIPSESS_CONN) {
		const struct sip_msg *msg=bevent_get_msg(event);
		if (!msg) return;
		bevent_stop(event);
		if (!ua_req_check_origin(ua,msg))
			(void)sip_treply(NULL,uag_sip(),msg,403,"Forbidden");
		else if (!phone_board_ready() || ((p->call || p->offhook) && !(p->cw && p->connected && !p->solo_hold && p->offhook && !p->other)))
			(void)sip_treply(NULL,uag_sip(),msg,486,"Busy Here");
		else {
			if (nf) for (unsigned i=0;i<nf;++i) prepare_identity(free_lines[i],msg);
			else prepare_identity(p,msg);
			if (select_audio(p,ua) || ua_accept(ua,msg)) warning("rbctl_voip: incoming session accept failed\n");
		}
		return;
	}
	if (!call) return;
	int err = 0;
	if (ev == BEVENT_CALL_INCOMING) {
		if (phone_board_ready() && p->cw && p->connected && !p->solo_hold && p->offhook && !p->other) {
			p->other=call; p->other_held=false;
			phone_wait_beep(p->line,true);
			if (p->cw_cid && p->cid_enabled && !send_cid(p)) (void)tone(p,31);
			return;
		}
		if (p->call || p->offhook || !phone_board_ready()) {
			ua_hangup(ua,call,486,"Busy Here"); return;
		}
		for (unsigned i=0;i<nf;++i) {
			struct phone_line *q=free_lines[i]; q->call=call; q->ua=ua;
			err=ring(q,call);
			if (err || p->hunt) break;
		}
	}
	else {
		p=NULL;
		for (unsigned i=0;i<PHONE_LINES;++i) {
			struct phone_line *q=&lines[i];
			if (q->call==call || q->other==call) {
				if (p && ev==BEVENT_CALL_CLOSED && q->ringing) release(q);
				else if (!p) p=q;
			}
		}
		if (!p) return;
	}
	if (ev==BEVENT_CALL_INCOMING) { if (err) hangup(p); return; }
	if (p->other==call && ev==BEVENT_CALL_CLOSED) {
		p->other=NULL; p->other_held=false; phone_wait_beep(p->line,false); return;
	}
	else if (p->call != call) return;
	else if (ev==BEVENT_CALL_REMOTE_SDP && p->resume_pending) {
		/* Let libre finish the hold transaction before sending a new offer. */
		tmr_start(&p->resume_timer,0,solo_resume,p);
	}
	else if (ev==BEVENT_CALL_REMOTE_SDP && p->connected && p->media && !p->solo_hold) {
		/* SIP codecs/ports belong to baresip. Refresh the unchanged local
		 * PCMA leg; never copy a PBX address or payload mapping into DD01. */
		err=media_record(p,11);
	}
	else if (ev == BEVENT_CALL_RINGING || ev == BEVENT_CALL_PROGRESS || ev == BEVENT_CALL_ESTABLISHED) {
		err=media_on(p);
		if (!err) phone_audio_mode(p->line,true,ev == BEVENT_CALL_RINGING);
		if (!err && ev == BEVENT_CALL_ESTABLISHED) p->connected = true;
	}
	else if (ev == BEVENT_CALL_CLOSED) {
		if (p->other && p->offhook) {
			bool held=p->other_held;
			p->call=p->other; p->other=NULL; p->other_held=false;
			phone_wait_beep(p->line,false);
			audio_set_source(call_audio(call),NULL,NULL);
			audio_set_player(call_audio(call),NULL,NULL);
			p->connected=held;
			err=held ? resume(p,p->call) : ua_answer(p->ua,p->call,VIDMODE_OFF);
			if (err) hangup(p);
			return;
		}
		/* Only a remote established-call close reaches here with an owner:
		 * local hangup clears p.call before ua_hangup emits CLOSED. */
		if (p->connected && p->offhook && p->open && phone_board_ready()) {
			phone_audio_mode(p->line,false,false);
			p->call = NULL; p->ua = NULL; p->connected = false;
			err = words(p,12);
			if (!err) {
				p->media = false;
				/* Keep only the local tone channel until HOOK_ON. Selector 1
				 * is the measured Italy busy cadence (see peer.md). */
				err = tone(p,p->disconnect_sel);
				if (!err) arm(p,TIME_WARN,p->warn_ms);
			}
		}
		else if (!p->connected && p->offhook && !p->ringing && phone_board_ready()) {
			unsigned code=call_scode(call); p->call=NULL; p->ua=NULL;
			err=progress(p,code);
		}
		else release(p);
	}
	if (err) { warning("rbctl_voip: call setup failed (%m)\n",err); hangup(p); }
}

void phone_line_lost(unsigned slot) { if (slot<PHONE_LINES) { hangup(&lines[slot]); voice_changed(); } }
void phone_board_lost(void) { for (unsigned i=0;i<PHONE_LINES;++i) phone_line_lost(i); }

/* Policy parsing is synchronous on the libre loop. During reload read the
 * candidate generation, without replacing baresip's process-wide config. */
static struct conf *candidate;
static struct conf *policy_conf(void) { return candidate ? candidate : conf_cur(); }

static int line_number(unsigned line,const char *name,int fallback,int lo,int hi,int *out)
{
	char key[64], value[32], *end=NULL;
	re_snprintf(key,sizeof(key),"rbctl_voip_%s%u",name,line);
	int err=conf_get_str(policy_conf(),key,value,sizeof(value));
	if (err==ENOENT) { *out=fallback; return 0; }
	if (err) return err;
	errno=0; long n=strtol(value,&end,10);
	if (errno || end==value || *end || n<lo || n>hi) return EINVAL;
	*out=(int)n; return 0;
}

static int policy_init(struct phone_line *p)
{
	char key[48], map[256]="[0-9*]+";
	re_snprintf(key,sizeof(key),"rbctl_voip_digitmap%u",p->line);
	int err=conf_get_str(policy_conf(),key,map,sizeof(map));
	if (err && err!=ENOENT) return err;
	if ((err=digitmap_parse(&p->map,map))) return err;
	struct { const char *key; unsigned *ms; int def; } timers[]={
		{"first_digit",&p->first_ms,15},{"short_digit",&p->short_ms,4},
		{"long_digit",&p->long_ms,10},{"offhook_warning",&p->warn_ms,60},
		{"howler_duration",&p->lock_ms,15}};
	for (size_t i=0;i<sizeof(timers)/sizeof(timers[0]);++i) {
		int n; err=line_number(p->line,timers[i].key,timers[i].def,1,600,&n);
		if (err) return err;
		*timers[i].ms=(unsigned)n*1000;
	}
	struct { const char *key; bool *value; int def; } flags[]={
		{"inband",&p->inband,0},{"require_registration",&p->require_reg,0},
		{"mwi",&p->mwi,1},{"call_waiting",&p->cw,1},{"cid",&p->cid_enabled,1},
		{"cw_cid",&p->cw_cid,0},{"hunt",&p->hunt,0},{"fax_profile",&p->fax_profile,0}};
	for (size_t i=0;i<sizeof(flags)/sizeof(flags[0]);++i) {
		int n; err=line_number(p->line,flags[i].key,flags[i].def,0,1,&n);
		if (err) return err;
		*flags[i].value=n!=0;
	}
	/* Raw adjustments only; limit configuration to the bench probe envelope. */
	err=line_number(p->line,"tx_gain",0,-6,6,&p->tx_gain);
	if (!err) err=line_number(p->line,"rx_gain",0,-6,6,&p->rx_gain);
	if (err) return err;
	struct { const char *key; unsigned *selector; int def; } tones[]={
		{"busy_sel",&p->busy_sel,1},{"congestion_sel",&p->congestion_sel,2},
		{"disconnect_sel",&p->disconnect_sel,1}};
	for (size_t i=0;i<sizeof(tones)/sizeof(tones[0]);++i) {
		int n; err=line_number(p->line,tones[i].key,tones[i].def,1,19,&n);
		if (err || !(n<=7 || n==9 || n==10 || (n>=15 && n<=17) || n==19)) return EINVAL;
		*tones[i].selector=(unsigned)n;
	}
	re_snprintf(key,sizeof(key),"rbctl_voip_alert_info%u",p->line);
	err=conf_get_str(policy_conf(),key,p->alert_match,sizeof(p->alert_match));
	if (err && err!=ENOENT) return err;
	int entry; err=line_number(p->line,"alert_entry",0,0,9,&entry);
	if (!err) p->alert_entry=(unsigned)entry;
	return err;
}
static int load_lines(struct phone_line *dst)
{
	for (unsigned i=0;i<PHONE_LINES;++i) {
		struct phone_line *p=&dst[i];
		p->line=i;
		tmr_init(&p->timer);
		tmr_init(&p->resume_timer);
		int err=policy_init(p);
		if (err) return err;
		int rc=conf_get_str(policy_conf(), i ? "rbctl_voip_account1" : "rbctl_voip_account0", p->aor,sizeof(p->aor));
		if (rc && rc!=ENOENT) return rc;
		p->enabled=*p->aor!='\0';
	}
	return 0;
}

int phone_init(unsigned line)
{
	tmr_init(&reload_timer);
	uint32_t managed=0;
	(void)conf_get_u32(conf_cur(),"rbctl_voip_managed",&managed);
	memset(lines,0,sizeof(lines));
	int err=load_lines(lines);
	if (err) return err;
	bool mapped=managed || lines[0].enabled || lines[1].enabled;
	if (!mapped) lines[line].enabled=true;
	err=phone_audio_init(mapped ? PHONE_LINES : line);
	if (!err) err=bevent_register(sip_event,NULL);
	if (err) phone_audio_close();
	return err;
}
void phone_close(void)
{
	tmr_cancel(&reload_timer); reload_pending=false;
	bevent_unregister(sip_event);
	phone_board_lost();
	phone_audio_close();
}

const char *voice_line_state(unsigned slot)
{
	const struct phone_line *p=&lines[slot];
	if (!p->enabled) return "disabled";
	if (p->recall) return "recall";
	if (p->ringing) return "ringing";
	if (p->solo_hold) return "held";
	if (p->other && !p->other_held) return "waiting";
	if (p->connected) return "connected";
	if (p->call) return "outgoing";
	if (p->collecting) return p->n ? "collecting" : "dialtone";
	if (p->timeout==TIME_LOCK) return "howler";
	if (p->timeout==TIME_WARN) return "disconnected";
	return p->offhook ? "lockout" : "idle";
}

struct call *voice_call(unsigned slot,bool secondary)
{
	return secondary ? lines[slot].other : lines[slot].call;
}

int voice_control(unsigned slot,const char *uri)
{
	if (slot>=PHONE_LINES || !lines[slot].enabled) return EINVAL;
	struct phone_line *p=&lines[slot];
	if (!uri) {
		if (!p->call) return ENOENT;
		hangup(p);
	}
	else {
		if (strncmp(uri,"sip:",4) && strncmp(uri,"sips:",5)) return EINVAL;
		if (strlen(uri)>512 || strpbrk(uri,"\r\n\t ")) return EINVAL;
		if (!phone_board_ready() || p->call || p->other) return EBUSY;
		struct ua *ua=mapped_ua(p);
		if (!ua || (p->require_reg && !ua_isregistered(ua))) return ENOENT;
		release(p);
		p->ua=ua;
		int err=open_slot(p);
		if (!err) err=select_audio(p,ua);
		if (!err) err=ua_connect(ua,&p->call,NULL,uri,VIDMODE_OFF);
		if (err) { release(p); return err; }
	}
	voice_changed();
	return 0;
}

/* The managed process owns its complete UA list. Stage new UAs without
 * registration; never tear down an active call or discard the previous UAs
 * on a parse/allocation error. */
struct ua_batch { struct ua *items[32]; unsigned n; };
static int stage_ua(const struct pl *pl,void *arg)
{
	struct ua_batch *b=arg;
	char line[1024];
	if (pl->l>=sizeof(line) || b->n>=RE_ARRAY_SIZE(b->items)) return E2BIG;
	pl_strcpy(pl,line,sizeof(line));
	int err=ua_alloc(&b->items[b->n],line);
	if (!err) ++b->n;
	return err;
}

int voice_reload(void)
{
	uint32_t managed=0;
	if (conf_get_u32(conf_cur(),"rbctl_voip_managed",&managed) || !managed) return ENOTSUP;
	char root[256], file[300];
	int err=conf_path_get(root,sizeof(root));
	if (err) return err;
	re_snprintf(file,sizeof(file),"%s/config",root);
	err=conf_alloc(&candidate,file);
	struct phone_line fresh[PHONE_LINES]={0};
	if (!err) err=load_lines(fresh);
	candidate=mem_deref(candidate);
	if (err) return err;
	struct ua_batch old={0}, next={0};
	for (struct le *le=uag_list()->head;le;le=le->next) {
		if (old.n>=RE_ARRAY_SIZE(old.items)) return E2BIG;
		old.items[old.n++]=le->data;
	}
	re_snprintf(file,sizeof(file),"%s/accounts",root);
	err=conf_parse(file,stage_ua,&next);
	for (unsigned i=0;!err && i<PHONE_LINES;++i) {
		if (!fresh[i].enabled) continue;
		bool found=false;
		for (unsigned j=0;j<next.n;++j)
			found |= !strcmp(fresh[i].aor,account_aor(ua_account(next.items[j])));
		if (!found) err=EINVAL;
	}
	if (err) {
		while (next.n) ua_destroy(next.items[--next.n]);
		return err;
	}
	if (lines_busy()) {
		/* Candidate parsed successfully; keep old UAs and all call ownership.
		 * The published generation is the latest requested configuration. */
		while (next.n) ua_destroy(next.items[--next.n]);
		if (!reload_pending) info("rbctl_voip: reload queued until lines idle\n");
		reload_pending=true; tmr_start(&reload_timer,250,apply_when_idle,NULL);
		return 0;
	}
	reload_pending=false; tmr_cancel(&reload_timer);
	for (unsigned i=0;i<PHONE_LINES;++i) for (unsigned j=0;j<PHONE_LINES;++j)
		if (fresh[i].enabled && fresh[i].mwi && lines[j].messages && !strcmp(lines[j].aor,fresh[i].aor))
			fresh[i].messages=true;
	while (old.n) ua_destroy(old.items[--old.n]);
	memcpy(lines,fresh,sizeof(lines));
	for (unsigned i=0;i<next.n;++i)
		if (account_regint(ua_account(next.items[i])) && ua_register(next.items[i]))
			warning("rbctl_voip: registration submission failed\n");
	voice_changed();
	return 0;
}
