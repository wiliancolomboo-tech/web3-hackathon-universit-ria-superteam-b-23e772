# Clone and enter directory
git clone https://github.com/superteam-brasil/bolsafi-protocol.git
cd bolsafi-protocol

# Fetch dependencies and compile Anchor program
anchor build

# Sync Program ID across lib.rs and Anchor.toml
anchor keys sync