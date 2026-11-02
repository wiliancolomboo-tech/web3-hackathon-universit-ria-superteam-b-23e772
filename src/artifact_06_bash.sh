# Configure Solana CLI to Devnet
solana config set --url https://api.devnet.solana.com

# Ensure deployer wallet is funded with Devnet SOL
solana airdrop 2

# Deploy the compiled shared object
anchor deploy --provider.cluster devnet

# Verify the deployed program on Solana Explorer:
# https://explorer.solana.com/address/PactoBRL11111111111111111111111111111111111?cluster=devnet