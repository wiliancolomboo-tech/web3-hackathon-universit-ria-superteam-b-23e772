# Set Solana CLI target to Devnet
solana config set --url https://api.devnet.solana.com

# Airdrop initial SOL for deployment fees
solana airdrop 2

# Build program with optimized release profile
anchor build

# Deploy directly via Anchor
anchor deploy --provider.cluster devnet

# Verify the deployed program on the network
solana program show Vortx11111111111111111111111111111111111111