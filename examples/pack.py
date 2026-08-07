#!/usr/bin/env python3
"""
TX payload packers for the 0x88B5 board protocol (P1).

Reverse-engineered from libcmm.so serializers:
  oal_dsl_lineObjToMsg  (FUN_00323584)  -> opcode 1 (dsl_config_up, 12 B)
  oal_atm_linkObjToMsg  (FUN_00324938)
  + oal_atm_qosObjToMsg (FUN_00324b84)  -> opcode 5 (atm_link_add,  24 B)
  oal_ptm_setVlanTag    (inline)        -> opcode 15 (ptm_link_add,  8 B)
  oal_*_delVlanTag      (inline)        -> opcode 6/16 (link_del,    3 B)

All multi-byte fields are big-endian on the wire (proto_postprocess applies
htonl/htons before TX). The descriptor starts at frame offset 0x18 (the byte the
struct calls bPayload_type is really payload byte 0). See docs/xdsl/payloads.md.

Run directly to execute the self-test:
    python3 examples/pack.py
"""
from __future__ import annotations
import struct

# ---------------------------------------------------------------------------
# Enum tables (extracted from libcmm.so globals)
# ---------------------------------------------------------------------------
MODULATION = {            # modulationTypes @ 0x3edc20
    "ADSL_ANSI_T1.413": 0, "ADSL_G.dmt": 1, "ADSL_G.lite": 2,
    "ADSL_G.dmt.bis": 3, "ADSL_2plus": 4, "ADSL_Multimode": 5,
    "VDSL2": 6, "Multimode": 7,
}
ANNEX = {                 # annexTypes @ 0x3edd20
    "Annex A": 0, "Annex B": 1, "Annex I": 2, "Annex M": 3,
    "Annex A/L": 4, "Annex A/L/M": 5, "Annex J": 6, "Annex B/J": 7,
    "Annex auto": 8,
}
VDSL2_PROFILE = {         # profiles @ 0x3eddb0  (bitmask)
    "8a": 0x001, "8b": 0x002, "8c": 0x004, "8d": 0x008,
    "12a": 0x010, "12b": 0x020, "17a": 0x040, "30a": 0x080, "35b": 0x100,
}
ATM_ENCAP = {"LLC": 0, "VCMUX": 1}                       # oal_atm_linkObjToMsg
ATM_LINK_TYPE = {"EoA": 0, "PPPoA": 6, "IPoA": 7}        # oal_atm_linkObjToMsg
ATM_QOS = {"UBR": 1, "CBR": 2, "VBR-nrt": 3, "VBR-rt": 4}  # oal_atm_qosObjToMsg


def _profile_bitmask(profiles) -> int:
    """OR together VDSL2 profile names (e.g. ['17a','30a']) -> bitmask."""
    bit = 0
    for p in profiles:
        bit |= VDSL2_PROFILE[p]
    return bit


# ---------------------------------------------------------------------------
# Opcode 1 — dsl_config_up  (12 bytes)
# ---------------------------------------------------------------------------
def pack_dsl_line(modulation: str, annex: str, bitswap: int = 0, sra: int = 0,
                  profiles=()) -> bytes:
    """Build the 12-byte DSL line config descriptor (opcode 1).

    The profile bitmask is only meaningful for modulation VDSL2/Multimode
    (codes 6/7); for ADSL modes it is sent as zero (matching libcmm, which only
    populates it when `*msg == 6 || *msg == 7`)."""
    mod = MODULATION[modulation]
    anx = ANNEX[annex]
    bitmask = _profile_bitmask(profiles) if mod in (6, 7) else 0
    return bytes([
        mod,            # [0] modulation code
        anx,            # [1] annex code
        bitswap & 0xff, # [2] X_TP_BitswapEnable (0/1)
        sra & 0xff,     # [3] X_TP_SRAEnable (0/1)
    ]) + struct.pack(">I", bitmask) + b"\x00\x00\x00\x00"  # [4..7] bitmask BE, [8..11] 0


# ---------------------------------------------------------------------------
# Opcode 5 — atm_link_add  (24 bytes)
# ---------------------------------------------------------------------------
def pack_atm_link(vpi: int, vci: int, encap: str, link_type: str,
                  qos: str, pcr: int = 0, scr: int = 0, mbs: int = 0,
                  vlan_id: int = 0, tag_enable: int = 0,
                  tag_vid: int = 0xffff, tag_pri: int = 0xff) -> bytes:
    """Build the 24-byte ATM link descriptor (opcode 5).

    vlan_id is the *local* vlan (dslVlan + 2000); tag_* are the 802.1Q tag the
    board applies to decapsulated frames. SCR/MBS are 0 unless qos is VBR."""
    cat = ATM_QOS[qos]
    enc = ATM_ENCAP[encap]
    lt = ATM_LINK_TYPE[link_type]
    return (bytes([
        cat & 0xff,         # [0]    QoS category (high 3 B overlap-cleared by VPI/VCI)
        vpi & 0xff,         # [1]    VPI
    ]) + struct.pack(">H", vci) +               # [2..3]  VCI
        struct.pack(">I", pcr) +                # [4..7]  peak cell rate
        struct.pack(">I", scr if qos.startswith("VBR") else 0) +  # [8..0xb] sustainable cell rate
        struct.pack(">I", mbs if qos.startswith("VBR") else 0) +  # [0xc..0xf] max burst size
        bytes([enc, lt]) +                      # [0x10] encap, [0x11] link type
        struct.pack(">H", vlan_id) +            # [0x12..0x13] local vlan id
        bytes([tag_enable & 0xff]) +            # [0x14] tag enable
        struct.pack(">H", tag_vid & 0xffff) +   # [0x15..0x16] tag vid
        bytes([tag_pri & 0xff]))                # [0x17] tag priority


# ---------------------------------------------------------------------------
# Opcode 15 — ptm_link_add  (8 bytes)
# ---------------------------------------------------------------------------
def pack_ptm_link(tag_enable: int, tag_vid: int, tag_pri: int,
                  vlan_id: int) -> bytes:
    """Build the 8-byte PTM/VDSL link descriptor (opcode 15)."""
    return (bytes([tag_enable & 0xff]) +              # [0] tag enable
        struct.pack(">H", tag_vid & 0xffff) +          # [1..2] tag vid
        struct.pack(">H", tag_pri & 0xffff) +          # [3..4] tag priority
        b"\x00" +                                      # [5] reserved
        struct.pack(">H", vlan_id & 0xffff))           # [6..7] local vlan id


# ---------------------------------------------------------------------------
# Opcodes 6 / 16 — atm_link_del / ptm_link_del  (3 bytes)
# ---------------------------------------------------------------------------
def pack_link_del(vlan_id: int, op_type: int = 3) -> bytes:
    """Build the 3-byte link-delete descriptor (opcode 6 or 16).

    op_type=3 means delete (the only value used by libcmm). The same layout
    serves ATM (opcode 6) and PTM (opcode 16)."""
    return struct.pack(">H", vlan_id & 0xffff) + bytes([op_type & 0xff])


# ---------------------------------------------------------------------------
# Self-test
# ---------------------------------------------------------------------------
def _selftest() -> None:
    # opcode 1: VDSL2, Annex B, profiles 17a + 30a  -> bitmask 0x40|0x80 = 0xc0
    dsl = pack_dsl_line("VDSL2", "Annex B", profiles=["17a", "30a"])
    assert len(dsl) == 12, len(dsl)
    assert dsl[0] == 6 and dsl[1] == 1, dsl[:2].hex()
    assert struct.unpack_from(">I", dsl, 4)[0] == 0xc0, dsl[4:8].hex()
    assert dsl[8:12] == b"\x00\x00\x00\x00"
    print(f"[ok] dsl_config_up (VDSL2/Annex B/17a+30a): {dsl.hex()}")

    # ADSL mode -> bitmask zero regardless of profiles arg
    dsl2 = pack_dsl_line("ADSL_2plus", "Annex A", profiles=["17a"])
    assert struct.unpack_from(">I", dsl2, 4)[0] == 0
    print(f"[ok] dsl_config_up (ADSL_2plus -> bitmask forced 0): {dsl2.hex()}")

    # opcode 5: ATM VPI=8 VCI=35 LLC/EoA UBR, local vlan 2035
    atm = pack_atm_link(8, 35, "LLC", "EoA", "UBR", pcr=1000, vlan_id=2035)
    assert len(atm) == 24, len(atm)
    assert atm[0] == 1 and atm[1] == 8, atm[:2].hex()           # UBR, VPI
    assert struct.unpack_from(">H", atm, 2)[0] == 35, atm[2:4].hex()  # VCI
    assert struct.unpack_from(">I", atm, 4)[0] == 1000           # PCR
    assert atm[0x10] == 0 and atm[0x11] == 0                     # LLC, EoA
    assert struct.unpack_from(">H", atm, 0x12)[0] == 2035        # vlan id
    print(f"[ok] atm_link_add (8/35 LLC/EoA/UBR vlan2035): {atm.hex()}")

    # VBR-nrt carries SCR/MBS; UBR/CBR zero them
    vbr = pack_atm_link(8, 35, "VCMUX", "PPPoA", "VBR-nrt", pcr=2000, scr=1000, mbs=50)
    assert struct.unpack_from(">I", vbr, 8)[0] == 1000 and struct.unpack_from(">I", vbr, 0xc)[0] == 50
    assert vbr[0x10] == 1 and vbr[0x11] == 6                      # VCMUX, PPPoA
    print(f"[ok] atm_link_add (VBR-nrt SCR/MBS carried): {vbr.hex()}")

    # opcode 15: PTM, local vlan 2001
    ptm = pack_ptm_link(tag_enable=1, tag_vid=100, tag_pri=2, vlan_id=2001)
    assert len(ptm) == 8, len(ptm)
    assert ptm[0] == 1 and struct.unpack_from(">H", ptm, 1)[0] == 100
    assert struct.unpack_from(">H", ptm, 6)[0] == 2001
    print(f"[ok] ptm_link_add (vlan2001): {ptm.hex()}")

    # opcode 6/16: link del
    dele = pack_link_del(2035)
    assert dele == bytes.fromhex("07f303"), dele.hex()           # 2035=0x07f3, type=3
    print(f"[ok] link_del (vlan2035): {dele.hex()}")


if __name__ == "__main__":
    _selftest()
    print("\nAll checks passed.")
