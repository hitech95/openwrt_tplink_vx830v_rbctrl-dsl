#!/usr/bin/env python3
"""
RX payload parsers for the 0x88B5 board protocol (P2).

Reverse-engineered from libcmm.so deserializers:
  oal_dsl_msgToLineObj           (FUN_00323d98)  -> opcode 2 reply (59 B)
  oal_dsl_msgToChannelObj        (FUN_003246a4)    (slices opcode 2)
  oal_dsl_msgToLineStatsObj      (FUN_00324770)    (slices opcode 2)
  oal_dsl_msgToChannelStatsTotObj(FUN_0032483c)  -> opcode 4 reply (28 B)

All multi-byte fields are big-endian on the wire. Metric field *names* are
inferred from TR-181 conventions (see docs/xdsl/responses.md); offsets/sizes/
byte-order are authoritative from the parser code.

Run directly to execute the self-test:
    python3 examples/unpack.py
"""
from __future__ import annotations
import struct
from pack import MODULATION, ANNEX, VDSL2_PROFILE  # reuse TX enum tables

LINK_STATUS = {
    0: "NoSignal", 1: "Up", 2: "NoSignal",
    3: "Initializing", 4: "EstablishingLink",
}

# Inferred line-metric names for the 12 uint32 at offsets 0x08..0x34.
# Inferred channel-stat names for the 6 uint32 in the opcode-4 reply.
# (See docs/xdsl/responses.md — names follow TR-181 order, offsets are fixed.)
_LINE_METRIC_NAMES = [
    ("down_curr_rate", 0x08), ("up_curr_rate", 0x0c),
    ("up_rate", 0x10), ("down_rate", 0x14),
    ("up_max_rate", 0x18), ("down_max_rate", 0x1c),
    ("up_snr_margin", 0x20), ("down_snr_margin", 0x24),
    ("up_attenuation", 0x28), ("down_attenuation", 0x2c),
    ("up_errors", 0x30), ("down_errors", 0x34),
]
_CHAN_STAT_NAMES = [
    ("receive_blocks", 0x04), ("receive_errors", 0x08),
    ("receive_discards", 0x0c), ("transmit_blocks", 0x10),
    ("transmit_errors", 0x14), ("transmit_discards", 0x18),
]


def _rev(table: dict, value, default=None):
    for k, v in table.items():
        if v == value:
            return k
    return default


def _profile_names(bitmask: int) -> list:
    return [name for name, bit in VDSL2_PROFILE.items() if bitmask & bit]


# ---------------------------------------------------------------------------
# Opcode 2 — dsl_get_line_obj reply (59 bytes)
# ---------------------------------------------------------------------------
def unpack_line_obj(data: bytes) -> dict:
    """Parse the 59-byte opcode-2 reply. Raises ValueError on short input."""
    if len(data) < 59:
        raise ValueError(f"line obj reply is 59 B, got {len(data)}")
    status = struct.unpack_from(">I", data, 0x00)[0]
    link = data[0x04]
    mod = data[0x05]
    annex = data[0x07]
    prof = struct.unpack_from(">H", data, 0x39)[0]
    metrics = {}
    for name, off in _LINE_METRIC_NAMES:
        metrics[name] = struct.unpack_from(">I", data, off)[0]
    return {
        "status": status,
        "link_status": LINK_STATUS.get(link, f"unknown({link})"),
        "link_status_code": link,
        "modulation": _rev(MODULATION, mod, f"unknown({mod})"),
        "modulation_code": mod,
        "annex": _rev(ANNEX, annex, f"unknown({annex})"),
        "annex_code": annex,
        "vdsl2_profiles": _profile_names(prof) if mod == 6 else None,
        "vdsl2_profile_bitmask": prof,
        "metrics": metrics,
    }


# ---------------------------------------------------------------------------
# Opcode 4 — dsl_get_channel_stats reply (28 bytes)
# ---------------------------------------------------------------------------
def unpack_channel_stats(data: bytes) -> dict:
    """Parse the 28-byte opcode-4 reply."""
    if len(data) < 28:
        raise ValueError(f"channel stats reply is 28 B, got {len(data)}")
    out = {"status": struct.unpack_from(">I", data, 0x00)[0]}
    for name, off in _CHAN_STAT_NAMES:
        out[name] = struct.unpack_from(">I", data, off)[0]
    return out


# ---------------------------------------------------------------------------
# Self-test
# ---------------------------------------------------------------------------
def _selftest() -> None:
    # build a synthetic 59-byte line reply: Up, VDSL2, Annex B, profile 17a+30a
    buf = bytearray(59)
    struct.pack_into(">I", buf, 0x00, 0)          # status OK
    buf[0x04] = 1                                  # Up
    buf[0x05] = 6                                  # VDSL2
    buf[0x07] = 1                                  # Annex B
    struct.pack_into(">H", buf, 0x39, 0x40 | 0x80) # 17a + 30a
    for _, off in _LINE_METRIC_NAMES:
        struct.pack_into(">I", buf, off, off * 10)  # dummy values
    line = unpack_line_obj(bytes(buf))
    assert line["status"] == 0
    assert line["link_status"] == "Up"
    assert line["modulation"] == "VDSL2"
    assert line["annex"] == "Annex B"
    assert set(line["vdsl2_profiles"]) == {"17a", "30a"}, line["vdsl2_profiles"]
    assert line["metrics"]["up_snr_margin"] == 0x20 * 10
    print(f"[ok] line_obj: Up/VDSL2/AnnexB/17a+30a, {len(line['metrics'])} metrics")

    # NoSignal line
    buf[0x04] = 0
    assert unpack_line_obj(bytes(buf))["link_status"] == "NoSignal"
    print("[ok] line_obj: NoSignal decoded")

    # channel stats (28 B)
    cbuf = bytearray(28)
    struct.pack_into(">I", cbuf, 0x00, 0)
    for _, off in _CHAN_STAT_NAMES:
        struct.pack_into(">I", cbuf, off, off)
    cs = unpack_channel_stats(bytes(cbuf))
    assert cs["status"] == 0
    assert cs["transmit_blocks"] == 0x10 and cs["receive_blocks"] == 0x04
    print(f"[ok] channel_stats: {len(cs) - 1} counters")


if __name__ == "__main__":
    _selftest()
    print("\nAll checks passed.")
