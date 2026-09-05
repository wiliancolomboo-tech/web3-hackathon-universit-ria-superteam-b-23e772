import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { BolsaFi } from "../target/types/bolsafi";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { expect } from "chai";

describe("BolsaFi Protocol Integration & Security Test Suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.BolsaFi as Program<BolsaFi>;

  const universityDean = anchor.web3.Keypair.generate();
  const academicAdvisor = anchor.web3.Keypair.generate();
  const student = anchor.web3.Keypair.generate();
  const maliciousAttacker = anchor.web3.Keypair.generate();

  let brzMint: anchor.web3.Pubkey;
  let deanTokenAccount: anchor.web3.Pubkey;
  let studentTokenAccount: anchor.web3.Pubkey;
  let poolPda: anchor.web3.Pubkey;
  let poolBump: number;
  let vaultPda: anchor.web3.Pubkey;
  let grantPda: anchor.web3.Pubkey;
  let grantBump: number;

  const poolId = new anchor.BN(42);
  const DEPOSIT_AMOUNT = new anchor.BN(10_000_000_000); // 10,000 BRZ
  const RATE_PER_SECOND = new anchor.BN(100_000);        // 0.1 BRZ / sec
  const DURATION_SECONDS = new anchor.BN(60);            // 60 sec stream
  const MILESTONE_BONUS = new anchor.BN(500_000_000);    // 500 BRZ milestone

  before(async () => {
    // AirDrop SOL to all signers
    for (const kp of [universityDean, academicAdvisor, student, maliciousAttacker]) {
      const sig = await provider.connection.requestAirdrop(kp.publicKey, 5 * anchor.web3.LAMPORTS_PER_SOL);
      const latestBlockhash = await provider.connection.getLatestBlockhash();
      await provider.connection.confirmTransaction({
        signature: sig,
        ...latestBlockhash,
      });
    }

    // Initialize Mock BRZ Mint
    brzMint = await createMint(
      provider.connection,
      universityDean,
      universityDean.publicKey,
      null,
      6
    );

    // Derive PDAs
    [poolPda, poolBump] = anchor.web3.Pubkey.findProgramAddressSync(
      [
        Buffer.from("endowment"),
        universityDean.publicKey.toBuffer(),
        poolId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    [vaultPda] = anchor.web3.Pubkey.findProgramAddressSync(
      [Buffer.from("vault"), poolPda.toBuffer()],
      program.programId
    );

    [grantPda, grantBump] = anchor.web3.Pubkey.findProgramAddressSync(
      [Buffer.from("grant"), poolPda.toBuffer(), student.publicKey.toBuffer()],
      program.programId
    );

    // Create Token Accounts
    deanTokenAccount = await createAccount(
      provider.connection,
      universityDean,
      brzMint,
      universityDean.publicKey
    );

    studentTokenAccount = await createAccount(
      provider.connection,
      student,
      brzMint,
      student.publicKey
    );

    // Mint initial funds to Dean
    await mintTo(
      provider.connection,
      universityDean,
      brzMint,
      deanTokenAccount,
      universityDean,
      20_000_000_000
    );
  });

  it("1. Initializes an Endowment Pool PDA & Escrow Vault", async () => {
    await program.methods
      .initializePool(poolId, poolBump)
      .accounts({
        pool: poolPda,
        tokenMint: brzMint,
        vault: vaultPda,
        authority: universityDean.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([universityDean])
      .rpc();

    const poolAccount = await program.account.endowmentPool.fetch(poolPda);
    expect(poolAccount.authority.toBase58()).to.equal(universityDean.publicKey.toBase58());
    expect(poolAccount.totalDeposited.toNumber()).to.equal(0);
    expect(poolAccount.totalAllocated.toNumber()).to.equal(0);
  });

  it("2. Deposits BRZ stablecoins into Endowment Vault", async () => {
    await program.methods
      .depositFunds(DEPOSIT_AMOUNT)
      .accounts({
        pool: poolPda,
        vault: vaultPda,
        sponsorTokenAccount: deanTokenAccount,
        authority: universityDean.publicKey,
        sponsor: universityDean.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([universityDean])
      .rpc();

    const poolAccount = await program.account.endowmentPool.fetch(poolPda);
    expect(poolAccount.totalDeposited.toNumber()).to.equal(DEPOSIT_AMOUNT.toNumber());

    const vaultInfo = await getAccount(provider.connection, vaultPda);
    expect(Number(vaultInfo.amount)).to.equal(DEPOSIT_AMOUNT.toNumber());
  });

  it("3. Creates a Programmatic Stream Grant with Cliff & Milestone for Student", async () => {
    const cliffSeconds = new anchor.BN(2); // 2-second cliff for test speed

    await program.methods
      .createStreamGrant(
        RATE_PER_SECOND,
        cliffSeconds,
        DURATION_SECONDS,
        MILESTONE_BONUS,
        grantBump
      )
      .accounts({
        pool: poolPda,
        grant: grantPda,
        student: student.publicKey,
        advisor: academicAdvisor.publicKey,
        authority: universityDean.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([universityDean])
      .rpc();

    const grantAccount = await program.account.grantStream.fetch(grantPda);
    expect(grantAccount.student.toBase58()).to.equal(student.publicKey.toBase58());
    expect(grantAccount.advisor.toBase58()).to.equal(academicAdvisor.publicKey.toBase58());
    expect(grantAccount.milestoneUnlocked).to.be.false;
  });

  it("4. Security Test: Malicious actor cannot attest academic milestones", async () => {
    try {
      await program.methods
        .attestMilestone()
        .accounts({
          grant: grantPda,
          advisor: maliciousAttacker.publicKey,
        })
        .signers([maliciousAttacker])
        .rpc();
      expect.fail("Unauthorized attestation should have been rejected!");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("UnauthorizedAdvisor");
    }
  });

  it("5. Advisor validates milestone and unlocks bonus", async () => {
    await program.methods
      .attestMilestone()
      .accounts({
        grant: grantPda,
        advisor: academicAdvisor.publicKey,
      })
      .signers([academicAdvisor])
      .rpc();

    const grantAccount = await program.account.grantStream.fetch(grantPda);
    expect(grantAccount.milestoneUnlocked).to.be.true;
  });

  it("6. Student claims accrued stream balance + milestone bonus", async () => {
    // Sleep 4 seconds to bypass cliff and accrue stream
    await new Promise((resolve) => setTimeout(resolve, 4000));

    const initialStudentBalance = (await getAccount(provider.connection, studentTokenAccount)).amount;

    await program.methods
      .claimStream()
      .accounts({
        pool: poolPda,
        grant: grantPda,
        vault: vaultPda,
        studentTokenAccount: studentTokenAccount,
        student: student.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([student])
      .rpc();

    const postClaimBalance = (await getAccount(provider.connection, studentTokenAccount)).amount;
    const diff = Number(postClaimBalance - initialStudentBalance);

    // Must have claimed at least 4 seconds worth + 500 BRZ milestone
    const minExpected = RATE_PER_SECOND.toNumber() * 3 + MILESTONE_BONUS.toNumber();
    expect(diff).to.be.at.least(minExpected);
  });

  it("7. Security Test: Re-claim immediately results in NoClaimableTokens error", async () => {
    try {
      await program.methods
        .claimStream()
        .accounts({
          pool: poolPda,
          grant: grantPda,
          vault: vaultPda,
          studentTokenAccount: studentTokenAccount,
          student: student.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([student])
        .rpc();
      expect.fail("Expected immediate zero-time claim to fail");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("NoClaimableTokens");
    }
  });

  it("8. Dean pauses grant due to academic freeze, blocking subsequent claims", async () => {
    await program.methods
      .togglePauseGrant()
      .accounts({
        pool: poolPda,
        grant: grantPda,
        authority: universityDean.publicKey,
      })
      .signers([universityDean])
      .rpc();

    const grant = await program.account.grantStream.fetch(grantPda);
    expect(grant.isPaused).to.be.true;

    // Sleep 2 seconds
    await new Promise((resolve) => setTimeout(resolve, 2000));

    try {
      await program.methods
        .claimStream()
        .accounts({
          pool: poolPda,
          grant: grantPda,
          vault: vaultPda,
          studentTokenAccount: studentTokenAccount,
          student: student.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([student])
        .rpc();
      expect.fail("Paused stream claim should have reverted!");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("GrantPaused");
    }
  });
});