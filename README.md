# Solana program

- Program id: `GXhVC4QomqL3Md7V578QLZ2P6egbrfXRjvrW9LspymZ9` (Solana mainnet)

## Verifiable build

```bash
solana-verify build --library-name prodpump
solana-verify get-executable-hash target/deploy/prodpump.so
solana-verify get-program-hash -um GXhVC4QomqL3Md7V578QLZ2P6egbrfXRjvrW9LspymZ9
```

The two hashes must be equal. The same build runs in this repository's "Verifiable build" workflow.
