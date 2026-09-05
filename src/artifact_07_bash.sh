# Switch to Solana Devnet
solana config set --url devnet

# Airdrop testnet SOL
solana airdrop 2

# Deploy BolsaFi Program to Devnet
anchor deploy --provider.cluster devnet

# Verify deployment on Solana Explorer
solana program show Bo1saFi1111111111111111111111111111111111111