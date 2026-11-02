# Switch to Solana Devnet
solana config set --url https://api.devnet.solana.com

# Fund deployer address via faucet
solana airdrop 2

# Build for Devnet release
anchor build --verifiable

# Deploy the binary
anchor deploy --provider.cluster devnet

# Verify the program on Solana Explorer / SolanaFM
solana program show <PROGRAM_ID>