// System types needed by libnl-tiny headers
#include <sys/socket.h>

// libnl-tiny core
#include <netlink/netlink.h>
#include <netlink/msg.h>
#include <netlink/attr.h>
#include <netlink/handlers.h>
#include <netlink/socket.h>

// libnl-tiny generic netlink
#include <netlink/genl/genl.h>
#include <netlink/genl/family.h>
#include <netlink/genl/ctrl.h>

// libnl-tiny micro-netlink
#include <unl.h>

// Kernel rtnetlink UAPI
#include <linux/rtnetlink.h>
#include <linux/if_link.h>
#include <linux/if_addr.h>
