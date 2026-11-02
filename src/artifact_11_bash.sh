# 1. Switch Solana CLI to Devnet
solana config set --url https://api.devnet.solana.com

# 2. Airdrop SOL to deployer wallet
solana airdrop 2

# 3. Deploy to Solana Devnet
anchor deploy --provider.cluster devnet

# 4. Verify Program on Explorer
echo "Program deployed successfully at: https://explorer.solana.com/address/$PROGRAM_ID?cluster=devnet"