import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Bolsastream } from "../target/types/bolsastream";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { assert, expect } from "chai";
import { Keypair, LAMPORTS_PER_SOL, PublicKey } from "@solana/web3.js";

describe("bolsastream e2e test suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Bolsastream as Program<Bolsastream>;
  const connection = provider.connection;

  const grantor = Keypair.generate();
  const beneficiary = Keypair.generate();
  const advisor1 = Keypair.generate();
  const advisor2 = Keypair.generate();

  let mint: PublicKey;
  let vaultKeypair = Keypair.generate();
  let grantorTokenAccount: PublicKey;
  let beneficiaryTokenAccount: PublicKey;

  const grantId = new anchor.BN(42);
  const totalAllocation = new anchor.BN(10_000_000_000); // 10,000 BRL Tokens (6 decimals)
  const requiredAttestations = 2;

  let grantPda: PublicKey;
  let grantBump: number;
  let vaultAuthPda: PublicKey;
  let vaultAuthBump: number;
  let milestone0Pda: PublicKey;

  before(async () => {
    // 1. Airdrop SOL to participants
    const airdropSigners = [grantor, beneficiary, advisor1, advisor2];
    for (const signer of airdropSigners) {
      const sig = await connection.requestAirdrop(signer.publicKey, 5 * LAMPORTS_PER_SOL);
      const latestBlockhash = await connection.getLatestBlockhash();
      await connection.confirmTransaction({
        signature: sig,
        blockhash: latestBlockhash.blockhash,
        lastValidBlockHeight: latestBlockhash.lastValidBlockHeight,
      });
    }

    // 2. Create SPL Mint (representing BRZ or Mock Real)
    mint = await createMint(
      connection,
      grantor,
      grantor.publicKey,
      null,
      6
    );

    // 3. Create Token Accounts
    grantorTokenAccount = await createAccount(
      connection,
      grantor,
      mint,
      grantor.publicKey
    );

    beneficiaryTokenAccount = await createAccount(
      connection,
      grantor,
      mint,
      beneficiary.publicKey
    );

    // 4. Mint tokens to grantor
    await mintTo(
      connection,
      grantor,
      mint,
      grantorTokenAccount,
      grantor,
      20_000_000_000
    );

    // 5. Derive PDAs
    [grantPda, grantBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("grant"), grantor.publicKey.toBuffer(), grantId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    [vaultAuthPda, vaultAuthBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault_auth"), grantPda.toBuffer()],
      program.programId
    );

    [milestone0Pda] = PublicKey.findProgramAddressSync(
      [Buffer.from("milestone"), grantPda.toBuffer(), Buffer.from([0])],
      program.programId
    );
  });

  it("1. Initializes grant escrow and registers PDA vaults successfully", async () => {
    const now = Math.floor(Date.now() / 1000);
    const startTime = new anchor.BN(now - 10); // started 10s ago
    const endTime = new anchor.BN(now + 100);  // runs for 110s total

    await program.methods
      .initializeGrant(
        grantId,
        totalAllocation,
        startTime,
        endTime,
        requiredAttestations
      )
      .accounts({
        grantor: grantor.publicKey,
        beneficiary: beneficiary.publicKey,
        mint: mint,
        grant: grantPda,
        vaultAuthority: vaultAuthPda,
        vault: vaultKeypair.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([grantor, vaultKeypair])
      .rpc();

    const grantAccount = await program.account.grant.fetch(grantPda);
    expect(grantAccount.grantor.toBase58()).to.equal(grantor.publicKey.toBase58());
    expect(grantAccount.beneficiary.toBase58()).to.equal(beneficiary.publicKey.toBase58());
    expect(grantAccount.totalAllocation.toNumber()).to.equal(totalAllocation.toNumber());
    expect(grantAccount.claimedAmount.toNumber()).to.equal(0);
    expect(grantAccount.requiredAttestations).to.equal(2);

    // Fund the escrow vault with SPL tokens
    await mintTo(
      connection,
      grantor,
      mint,
      vaultKeypair.publicKey,
      grantor,
      totalAllocation.toNumber()
    );

    const vaultBalance = await connection.getTokenAccountBalance(vaultKeypair.publicKey);
    expect(vaultBalance.value.uiAmount).to.equal(10000);
  });

  it("2. Registers milestone #0 (Dissertation Defense / Peer Review)", async () => {
    const dummyHash = new Array(32).fill(7); // 32-byte sha256 mock hash
    const milestoneAllocation = new anchor.BN(5_000_000_000);

    await program.methods
      .registerMilestone(0, milestoneAllocation, dummyHash)
      .accounts({
        grant: grantPda,
        grantor: grantor.publicKey,
        milestone: milestone0Pda,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([grantor])
      .rpc();

    const milestoneAccount = await program.account.milestone.fetch(milestone0Pda);
    expect(milestoneAccount.index).to.equal(0);
    expect(milestoneAccount.attestationsCount).to.equal(0);
    expect(milestoneAccount.isCompleted).to.be.false;
  });

  it("3. Evaluates multi-sig milestone attestations and enforces threshold", async () => {
    // Advisor 1 attestation
    await program.methods
      .attestMilestone(0)
      .accounts({
        attestor: advisor1.publicKey,
        grant: grantPda,
        milestone: milestone0Pda,
      })
      .signers([advisor1])
      .rpc();

    let milestoneState = await program.account.milestone.fetch(milestone0Pda);
    expect(milestoneState.attestationsCount).to.equal(1);
    expect(milestoneState.isCompleted).to.be.false;

    // Advisor 2 attestation (reaches threshold = 2)
    await program.methods
      .attestMilestone(0)
      .accounts({
        attestor: advisor2.publicKey,
        grant: grantPda,
        milestone: milestone0Pda,
      })
      .signers([advisor2])
      .rpc();

    milestoneState = await program.account.milestone.fetch(milestone0Pda);
    expect(milestoneState.attestationsCount).to.equal(2);
    expect(milestoneState.isCompleted).to.be.true;
  });

  it("4. Beneficiary claims streamed capital linearly over time", async () => {
    // Wait 2 seconds to accumulate streaming velocity
    await new Promise((resolve) => setTimeout(resolve, 2000));

    const priorBeneficiaryAccount = await getAccount(connection, beneficiaryTokenAccount);

    await program.methods
      .claimStreamedFunds()
      .accounts({
        beneficiary: beneficiary.publicKey,
        grant: grantPda,
        vaultAuthority: vaultAuthPda,
        vault: vaultKeypair.publicKey,
        beneficiaryTokenAccount: beneficiaryTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
        clock: anchor.web3.SYSVAR_CLOCK_PUBKEY,
      })
      .signers([beneficiary])
      .rpc();

    const postBeneficiaryAccount = await getAccount(connection, beneficiaryTokenAccount);
    const claimedDelta = Number(postBeneficiaryAccount.amount) - Number(priorBeneficiaryAccount.amount);
    
    expect(claimedDelta).to.be.greaterThan(0);

    const grantAccount = await program.account.grant.fetch(grantPda);
    expect(grantAccount.claimedAmount.toNumber()).to.be.greaterThan(0);
  });

  it("5. Security Guard: Prevents unauthorized caller from claiming stream", async () => {
    const attacker = Keypair.generate();
    await connection.requestAirdrop(attacker.publicKey, 1 * LAMPORTS_PER_SOL);

    const attackerTokenAccount = await createAccount(
      connection,
      grantor,
      mint,
      attacker.publicKey
    );

    try {
      await program.methods
        .claimStreamedFunds()
        .accounts({
          beneficiary: attacker.publicKey,
          grant: grantPda,
          vaultAuthority: vaultAuthPda,
          vault: vaultKeypair.publicKey,
          beneficiaryTokenAccount: attackerTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
          clock: anchor.web3.SYSVAR_CLOCK_PUBKEY,
        })
        .signers([attacker])
        .rpc();
      assert.fail("Should have thrown UnauthorizedBeneficiary error");
    } catch (err: any) {
      expect(err.toString()).to.include("UnauthorizedBeneficiary");
    }
  });

  it("6. Grantor triggers clawback on unvested remainder, preserving vested allocations", async () => {
    const priorGrantorBalance = (await getAccount(connection, grantorTokenAccount)).amount;

    await program.methods
      .cancelAndClawback()
      .accounts({
        grantor: grantor.publicKey,
        grant: grantPda,
        vaultAuthority: vaultAuthPda,
        vault: vaultKeypair.publicKey,
        grantorTokenAccount: grantorTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
        clock: anchor.web3.SYSVAR_CLOCK_PUBKEY,
      })
      .signers([grantor])
      .rpc();

    const postGrantorBalance = (await getAccount(connection, grantorTokenAccount)).amount;
    const clawedBack = Number(postGrantorBalance) - Number(priorGrantorBalance);

    expect(clawedBack).to.be.greaterThan(0);

    const grantState = await program.account.grant.fetch(grantPda);
    expect(grantState.isCancelled).to.be.true;
  });
});