# 1. Clone the repository and navigate to root directory
cd pactobrl

# 2. Install Node dependencies
yarn install

# 3. Synchronize program keys (generates deterministic target keypair)
anchor keys sync

# 4. Build the Anchor Solana program
anchor build

# 5. Run the complete end-to-end integration and security test suite on localnet
anchor test