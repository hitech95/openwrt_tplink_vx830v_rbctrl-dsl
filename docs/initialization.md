

The startup sequence of `remote_board`, from ELF entry to the running event loop.

## Entry → main

ELF `_start` is at `entry`. Per the glibc ABI, it invokes:

```c
__libc_start_main(main, argc, argv, _init, _fini, 0);
```

where `main` is at.

## main()

```mermaid
flowchart TD
    A["main(argc, argv)"] --> B["getopt('hi:')"]
    B -->|"'-h'"| H["print help, exit(0)"]
    B -->|"'-i &lt;iface&gt;'"| I["set_interface_name(optarg)<br/>strncpy into g_szIfaceName"]
    B -->|"'default / end'"| C["raw_socket_init()<br/>0x88B6 control socket"]
    C -->|"ret &lt; 0"| X1["exit(1)"]
    C -->|"ret >= 0"| D["init conn-table entry"]
    D --> E["cmm_init(&conn_table[n])<br/>msg_init + msg_srvInit(0x3b)"]
    E -->|"ret != 0"| X2["exit(1)"]
    E -->|"ret == 0"| F["conn_count++"]
    F --> G["cmm_event_loop()<br/>infinite select/msg_recv"]
    G --> Z["(unreachable) cleanup"]
    style X1 fill:#5a1a1a,color:#fff
    style X2 fill:#5a1a1a,color:#fff
```

### Command-line options

Parsed with `getopt(argc, argv, "hi:")`:

| Option | Argument | Handler | Effect |
|--------|----------|---------|--------|
| `-h` | — | `print_usage` | Print usage and `exit(0)` |
| `-i` | `<iface>` | `set_interface_name` | `strncpy(g_szIfaceName, optarg, 16)` |

`-i` overrides the default interface name `"lan0.500"`. On a real board this is
typically `eth0.500` or similar. The `.NNN` suffix selects the VLAN.

### raw_socket_init()

Opens the **control-plane** socket (EtherType `0x88B6`). See
[network.md](network.md) for details. On failure prints one of:

- `"socket init failed"`
- `"set socket filter failed"`
- `"bind interface failed"`

and returns `-1` → `main` exits.

### cmm_init()

Initializes the libcmm context. See [libcmm.md](libcmm.md).

### cmm_event_loop()

Never returns — runs the `select()`/`msg_recv` dispatch loop. See
[libcmm.md](libcmm.md#event-loop) and [dispatch.md](commands/dispatch.md).

## Startup sequence (timeline)

```mermaid
sequenceDiagram
    participant M as main
    participant RS as raw_socket_init
    participant CI as cmm_init
    participant L as libcmm.so
    participant K as Linux kernel

    M->>M: getopt -i "lan0.500"
    M->>RS: raw_socket_init()
    RS->>K: socket(AF_PACKET, SOCK_RAW, 0x88B6)
    RS->>K: setsockopt(SO_BROADCAST)
    RS->>K: ioctl(SIOCGIFHWADDR)
    RS->>K: setsockopt(SO_ATTACH_FILTER)
    RS->>K: bind(sockaddr_ll, ifindex(lan0.500))
    RS-->>M: fd (>=0)
    M->>CI: cmm_init(&conn_table[0])
    CI->>CI: memset(ctx, 0, 0xE0)
    CI->>L: msg_init(ctx)
    CI->>L: msg_srvInit(0x3B, ctx)
    L-->>CI: 0 (ok) / -1 (fail)
    CI-->>M: store ctx + callback (cmm_msg_handler)
    M->>M: cmm_event_loop()  %% infinite
```
