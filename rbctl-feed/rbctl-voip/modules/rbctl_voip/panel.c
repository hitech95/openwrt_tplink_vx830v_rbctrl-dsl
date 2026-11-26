/* SPDX-License-Identifier: GPL-2.0-or-later */
#include <errno.h>
#include <spawn.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>
#include <re.h>
#include <baresip.h>
#include "phone.h"
#include "panel.h"

static struct tmr timer;
static int fd=-1;
static pid_t pid=-1;
static const char *last;

static void poll_panel(void *unused)
{
	(void)unused;
	const char *s=phone_panel_state();
	if (!last || strcmp(s,last)) {
		ssize_t n=send(fd,s,strlen(s),MSG_NOSIGNAL|MSG_DONTWAIT);
		if (n==(ssize_t)strlen(s)) last=s;
		else if (n>=0 || (errno!=EAGAIN && errno!=EINTR)) {
			warning("rbctl_voip: LED worker unavailable\n"); return;
		}
	}
	tmr_start(&timer,250,poll_panel,NULL);
}

void panel_start(void)
{
	const char *path="/usr/libexec/rbctl-fxs-notify";
#ifdef RBCTL_HARNESS
	path=getenv("RBCTL_PANEL_FIXTURE");
#endif
	if (!path || access(path,X_OK)) return;
	int sv[2];
	if (socketpair(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0,sv)) return;
	posix_spawn_file_actions_t fa;
	int err=posix_spawn_file_actions_init(&fa);
	if (!err) {
		err=posix_spawn_file_actions_adddup2(&fa,sv[1],STDIN_FILENO);
		char *const argv[]={(char *)path,NULL};
		char *const envp[]={"PATH=/usr/sbin:/usr/bin:/sbin:/bin",NULL};
		if (!err) err=posix_spawn(&pid,path,&fa,NULL,argv,envp);
		posix_spawn_file_actions_destroy(&fa);
	}
	close(sv[1]);
	if (err) { close(sv[0]); warning("rbctl_voip: LED worker (%m)\n",err); return; }
	fd=sv[0]; last=NULL; tmr_init(&timer); poll_panel(NULL);
}

void panel_close(void)
{
	if (fd<0) return;
	tmr_cancel(&timer); close(fd); fd=-1;
	/* Drain the last transition and OFF before a reloaded module starts. */
	while (waitpid(pid,NULL,0)<0 && errno==EINTR) {}
	pid=-1; last=NULL;
}
