# Switch to Solana Devnet
solana config set --url https://api.devnet.solana.com

# Fund deployer account
solana airdrop 2

# Sync program ID with compiled keypair
anchor keys sync

# Build and deploy to Devnet
anchor build
anchor deploy --provider.cluster devnet