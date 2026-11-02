# Generate Anchor keypair
solana-keygen new --no-bip39-passphrase -o target/deploy/bolsafi-keypair.json --force

# Sync program ID into Rust source and configuration
PROGRAM_ID=$(solana address -k target/deploy/bolsafi-keypair.json)
sed -i.bak "s/BoLSaFi111111111111111111111111111111111111/$PROGRAM_ID/g" programs/bolsafi/src/lib.rs
sed -i.bak "s/BoLSaFi111111111111111111111111111111111111/$PROGRAM_ID/g" Anchor.toml

# Compile program to BPF target
anchor build