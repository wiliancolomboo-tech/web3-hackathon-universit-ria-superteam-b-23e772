# Set Solana CLI to Localnet or Devnet
solana config set --url localhost

# Generate deployment keypair if not already present
solana-keygen new --no-bip39-passphrase -o ~/.config/solana/id.json --force