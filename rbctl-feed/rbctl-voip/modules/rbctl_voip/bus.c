/* SPDX-License-Identifier: GPL-2.0-or-later */
#include <errno.h>
#include <string.h>
#include <re.h>
#include <baresip.h>
/* Both libraries declare an unrelated static inline list_move. */
#define list_move ubox_list_move
#include <libubus.h>
#include <libubox/blobmsg.h>
#undef list_move
#include "phone.h"
#include "manage.h"

/* All snapshots and callbacks run on the same libre loop as the line FSM. */
static struct ubus_context *bus;
static struct re_fhs *watch;
static struct tmr retry;
static const char *previous_board, *previous_line[PHONE_LINES];
static bool online;
static char socket_path[108];

static void add_line(struct blob_buf *b, unsigned i)
{
	blobmsg_add_u32(b,"line",i);
	blobmsg_add_u32(b,"slot",i);
	blobmsg_add_string(b,"state",voice_line_state(i));
}

static void add_board(struct blob_buf *b)
{
	blobmsg_add_string(b,"state",voice_board_state());
	blobmsg_add_string(b,"peer_mac",voice_board_peer());
}

static void add_dialog(struct blob_buf *b,struct call *c)
{
	const struct aucodec *codec=audio_codec(call_audio(c),true);
	blobmsg_add_string(b,"id",call_id(c) ? call_id(c) : "");
	blobmsg_add_string(b,"peer_uri",call_peeruri(c) ? call_peeruri(c) : "");
	blobmsg_add_string(b,"codec",codec ? codec->name : "");
	blobmsg_add_u32(b,"duration",call_duration(c));
}

static int inspect(struct ubus_context *ctx,struct ubus_object *obj,
	struct ubus_request_data *req,const char *method,struct blob_attr *msg)
{
	(void)obj; (void)msg;
	struct blob_buf b={0};
	blob_buf_init(&b,0);
	if (!strcmp(method,"status")) {
		blobmsg_add_u8(&b,"reload_pending",voice_reload_pending());
		void *t=blobmsg_open_table(&b,"board");
		add_board(&b); blobmsg_close_table(&b,t);
		void *a=blobmsg_open_array(&b,"accounts");
		for (struct le *le=uag_list()->head;le;le=le->next) {
			struct ua *ua=le->data;
			t=blobmsg_open_table(&b,NULL);
			blobmsg_add_string(&b,"aor",account_aor(ua_account(ua)));
			blobmsg_add_u8(&b,"registered",ua_isregistered(ua));
			blobmsg_close_table(&b,t);
		}
		blobmsg_close_array(&b,a);
	}
	bool calls=!strcmp(method,"calls");
	void *a=blobmsg_open_array(&b,calls ? "calls" : "lines");
	for (unsigned i=0;i<PHONE_LINES;++i) {
		for (unsigned j=0;j<(calls ? 2u : 1u);++j) {
			struct call *c=voice_call(i,j!=0);
			if (calls && !c) continue;
			void *t=blobmsg_open_table(&b,NULL);
			add_line(&b,i);
			if (c) { add_dialog(&b,c); blobmsg_add_u8(&b,"secondary",j!=0); }
			blobmsg_close_table(&b,t);
		}
	}
	blobmsg_close_array(&b,a);
	int rc=ubus_send_reply(ctx,req,b.head);
	blob_buf_free(&b);
	return rc;
}

static const struct blobmsg_policy args[]={
	{.name="line",.type=BLOBMSG_TYPE_INT32},
	{.name="uri",.type=BLOBMSG_TYPE_STRING},
};

static int control(struct ubus_context *ctx,struct ubus_object *obj,
	struct ubus_request_data *req,const char *method,struct blob_attr *msg)
{
	(void)ctx; (void)obj; (void)req;
	int err;
	if (!strcmp(method,"reload")) err=voice_reload();
	else {
		struct blob_attr *tb[2]={0};
		if (msg) blobmsg_parse(args,2,tb,blob_data(msg),blob_len(msg));
		bool dial=!strcmp(method,"dial");
		if (!tb[0] || (dial && !tb[1])) return UBUS_STATUS_INVALID_ARGUMENT;
		err=voice_control(blobmsg_get_u32(tb[0]),dial ? blobmsg_get_string(tb[1]) : NULL);
	}
	switch (err) {
	case 0: return UBUS_STATUS_OK;
	case EINVAL: return UBUS_STATUS_INVALID_ARGUMENT;
	case ENOENT: return UBUS_STATUS_NOT_FOUND;
	case ENOTSUP: return UBUS_STATUS_NOT_SUPPORTED;
	default: return UBUS_STATUS_SYSTEM_ERROR;
	}
}

static const struct ubus_method methods[]={
	UBUS_METHOD_NOARG("status",inspect), UBUS_METHOD_NOARG("lines",inspect),
	UBUS_METHOD_NOARG("calls",inspect), UBUS_METHOD("dial",control,args),
	{.name="hangup",.handler=control,.policy=args,.n_policy=1},
	UBUS_METHOD_NOARG("reload",control),
};
static struct ubus_object_type type=UBUS_OBJECT_TYPE("voip",methods);
static struct ubus_object object={.name="voip",.type=&type,.methods=methods,
	.n_methods=ARRAY_SIZE(methods)};

void voice_changed(void)
{
	if (!online) return;
	struct blob_buf b={0};
	const char *s=voice_board_state();
	if (!previous_board || strcmp(previous_board,s)) {
		previous_board=s;
		blob_buf_init(&b,0); add_board(&b); ubus_send_event(bus,"voip.board",b.head);
	}
	for (unsigned i=0;i<PHONE_LINES;++i) {
		s=voice_line_state(i);
		if (previous_line[i] && !strcmp(previous_line[i],s)) continue;
		previous_line[i]=s;
		blob_buf_init(&b,0); add_line(&b,i); ubus_send_event(bus,"voip.line",b.head);
	}
	blob_buf_free(&b);
}

static void event(enum bevent_ev ev,struct bevent *e,void *arg)
{
	(void)arg;
	if (!online) return;
	struct blob_buf b={0};
	struct ua *ua=bevent_get_ua(e);
	struct call *c=bevent_get_call(e);
	const char *name=NULL;
	if (ua && ev<=BEVENT_UNREGISTERING) name="voip.reg";
	else if (c && ((ev>=BEVENT_CALL_INCOMING && ev<=BEVENT_CALL_CLOSED) ||
		ev==BEVENT_CALL_HOLD || ev==BEVENT_CALL_RESUME)) name="voip.call";
	if (name) {
		blob_buf_init(&b,0);
		blobmsg_add_string(&b,"event",bevent_str(ev));
		if (ua) blobmsg_add_string(&b,"aor",account_aor(ua_account(ua)));
		if (c) add_dialog(&b,c);
		ubus_send_event(bus,name,b.head);
		blob_buf_free(&b);
	}
	voice_changed();
}

static void connect_bus(void *unused);

static void lost(struct ubus_context *ctx)
{
	(void)ctx;
	online=false;
	if (watch) watch=fd_close(watch);
	tmr_start(&retry,1000,connect_bus,NULL);
}

static void readable(int flags,void *arg)
{
	(void)arg;
	if (flags & FD_EXCEPT) lost(bus);
	else ubus_handle_event(bus);
}

static void connect_bus(void *unused)
{
	(void)unused;
	if (bus) ubus_free(bus);
	bus=ubus_connect(*socket_path ? socket_path : NULL);
	object.id=type.id=0;
	if (bus) bus->connection_lost=lost;
	if (!bus || ubus_add_object(bus,&object) || fd_listen(&watch,bus->sock.fd,FD_READ,readable,NULL)) {
		lost(bus);
		return;
	}
	previous_board=NULL; memset(previous_line,0,sizeof(previous_line));
	online=true; voice_changed();
}

int voice_bus_start(void)
{
	uint32_t managed=0;
	if (conf_get_u32(conf_cur(),"rbctl_voip_managed",&managed) || !managed) return 0;
	tmr_init(&retry);
	int rc=conf_get_str(conf_cur(),"rbctl_voip_ubus_socket",socket_path,sizeof(socket_path));
	if (rc && rc!=ENOENT) return rc;
	int err=bevent_register(event,NULL);
	if (!err) connect_bus(NULL);
	return err;
}

void voice_bus_close(void)
{
	online=false;
	bevent_unregister(event);
	tmr_cancel(&retry);
	if (watch) watch=fd_close(watch);
	if (bus) { ubus_free(bus); bus=NULL; }
}
