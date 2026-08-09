# fwextract

Standalone CLI that extracts and optionally decrypts the remote-board (xDSL)
firmware from a TP-Link multi-image container (`*.bin`).

## Build

```sh
cargo build --release                  # split only
cargo build --release --features decrypt   # split + AES decryption
```

The `decrypt` feature pulls in `num-bigint-dig` (RSA), `sha2` (PSS, MGF1),
`aes` + `cbc` (AES-128-CBC) and `base64` (PUBLICKEYBLOB decoding).

## Usage

```sh
# Split only — output is still AES-encrypted
fwextract firmware.bin --split output/

# Decrypt and split (requires --features decrypt)
python3 ../examples/extract_rsa_keys.py libcmm.so --output-dir keys/
fwextract --decrypt --rsa-key keys/rsa2048_pub.bin firmware.bin --split output/
```

Produces `output/host_kernel.bin` (host UBI image) and
`output/remote_board.bin` (the `2RDH` / `tclinux.trx` remote-board firmware).

### Options

| Flag | Description |
|------|-------------|
| `-o, --output <PATH>` | Write remote board image to `PATH` |
| `-a, --all` | Also extract the host kernel image |
| `-d, --decrypt` | Decrypt firmware in place (requires `--rsa-key`) |
| `-k, --rsa-key <PATH>` | RSA public key blob (CryptoAPI `PUBLICKEYBLOB`) |
| `-s, --split <DIR>` | Write `host_kernel.bin` + `remote_board.bin` to `DIR` |
| `-h, --help` | Show help |

## How decryption works

The container signs the firmware with **RSA-2048 PSS** and encrypts it with
**AES-128-CBC**. The AES key + IV are embedded in the PSS signature's salt,
so they are recoverable using **only the RSA public key** during signature
verification — no private key is required to decrypt existing firmware.

Full container layout, call-chain analysis, and the key-extraction algorithm
are documented in [../docs/firmware_encryption.md](../docs/firmware_encryption.md).
A Python reference implementation lives at
[../examples/decrypt_fw.py](../examples/decrypt_fw.py).

## License

GPL-2.0-only
