# 1. Switch CLI to Solana Devnet
solana config set --url https://api.devnet.solana.com

# 2. Airdrop Devnet SOL for deployment fees
solana airdrop 3

# 3. Deploy to Solana Devnet
anchor deploy --provider.cluster devnet

# 4. Verify deployment
solana program show <PROGRAM_ID>