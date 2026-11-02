import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { BolsaFi } from "../target/types/bolsafi";
import {
  TOKEN_PROGRAM_ID,
  createMint,
  createAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import { expect } from "chai";

describe("BolsaFi Protocol Integration & Security Matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.BolsaFi as Program<BolsaFi>;

  let mint: anchor.web3.PublicKey;
  let sponsorTokenAccount: anchor.web3.PublicKey;
  let studentTokenAccount: anchor.web3.PublicKey;

  const sponsor = anchor.web3.Keypair.generate();
  const student = anchor.web3.Keypair.generate();
  const advisor = anchor.web3.Keypair.generate();
  const maliciousActor = anchor.web3.Keypair.generate();

  const grantId = new anchor.BN(1001);
  const totalGrantAmount = new anchor.BN(10_000_000_000); // 10,000 BRZ (6 decimals)
  const streamPercentage = 40; // 40% linear stream, 60% milestones
  const totalMilestones = 3;

  let grantPda: anchor.web3.PublicKey;
  let grantBump: number;
  let vaultPda: anchor.web3.PublicKey;
  let vaultBump: number;

  before(async () => {
    // Fund test wallets with SOL for rent & tx fees
    for (const kp of [sponsor, student, advisor, maliciousActor]) {
      const airdropSig = await provider.connection.requestAirdrop(
        kp.publicKey,
        5 * anchor.web3.LAMPORTS_PER_SOL
      );
      const latestBlockhash = await provider.connection.getLatestBlockhash();
      await provider.connection.confirmTransaction({
        signature: airdropSig,
        ...latestBlockhash,
      });
    }

    // Create synthetic BRZ Token Mint
    mint = await createMint(
      provider.connection,
      sponsor,
      sponsor.publicKey,
      null,
      6
    );

    sponsorTokenAccount = await createAccount(
      provider.connection,
      sponsor,
      mint,
      sponsor.publicKey
    );

    studentTokenAccount = await createAccount(
      provider.connection,
      student,
      mint,
      student.publicKey
    );

    // Mint funds to Sponsor
    await mintTo(
      provider.connection,
      sponsor,
      mint,
      sponsorTokenAccount,
      sponsor,
      20_000_000_000
    );

    // Derive PDAs
    [grantPda, grantBump] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("grant_state"),
        sponsor.publicKey.toBuffer(),
        grantId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    [vaultPda, vaultBump] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("grant_vault"), grantPda.toBuffer()],
      program.programId
    );
  });

  it("1. Successfully initializes grant and locks tokens in Escrow PDA", async () => {
    const now = Math.floor(Date.now() / 1000);
    const startTime = new anchor.BN(now - 100); // 100s in past for immediate streaming test
    const endTime = new anchor.BN(now + 900); // 1000s total duration

    const tx = await program.methods
      .initializeGrant(
        grantId,
        totalGrantAmount,
        streamPercentage,
        startTime,
        endTime,
        totalMilestones
      )
      .accounts({
        sponsor: sponsor.publicKey,
        beneficiary: student.publicKey,
        advisor: advisor.publicKey,
        mint: mint,
        grantAccount: grantPda,
        escrowVault: vaultPda,
        sponsorTokenAccount: sponsorTokenAccount,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([sponsor])
      .rpc();

    expect(tx).to.be.a("string");

    const vaultInfo = await getAccount(provider.connection, vaultPda);
    expect(Number(vaultInfo.amount)).to.equal(10_000_000_000);

    const grantState = await program.account.grantAccount.fetch(grantPda);
    expect(grantState.sponsor.toBase58()).to.equal(sponsor.publicKey.toBase58());
    expect(grantState.beneficiary.toBase58()).to.equal(student.publicKey.toBase58());
    expect(grantState.streamAmount.toNumber()).to.equal(4_000_000_000);
    expect(grantState.milestoneAmount.toNumber()).to.equal(6_000_000_000);
  });

  it("2. Blocks unauthorized actor from approving milestone", async () => {
    try {
      await program.methods
        .approveMilestone(0)
        .accounts({
          reviewer: maliciousActor.publicKey,
          grantAccount: grantPda,
        })
        .signers([maliciousActor])
        .rpc();
      expect.fail("Expected transaction to fail with UnauthorizedReviewer");
    } catch (err: any) {
      expect(err.toString()).to.include("UnauthorizedReviewer");
    }
  });

  it("3. Allows verified Advisor to approve academic milestone #0", async () => {
    await program.methods
      .approveMilestone(0)
      .accounts({
        reviewer: advisor.publicKey,
        grantAccount: grantPda,
      })
      .signers([advisor])
      .rpc();

    const grantState = await program.account.grantAccount.fetch(grantPda);
    expect(grantState.milestonesApproved[0]).to.be.true;
    expect(grantState.milestonesApproved[1]).to.be.false;
  });

  it("4. Prevents duplicate approval of already completed milestone", async () => {
    try {
      await program.methods
        .approveMilestone(0)
        .accounts({
          reviewer: advisor.publicKey,
          grantAccount: grantPda,
        })
        .signers([advisor])
        .rpc();
      expect.fail("Expected transaction to fail on double approval");
    } catch (err: any) {
      expect(err.toString()).to.include("MilestoneInvalidOrAlreadyApproved");
    }
  });

  it("5. Allows scholar to claim streaming stipend + approved milestone tranche", async () => {
    const preStudentBalance = (
      await getAccount(provider.connection, studentTokenAccount)
    ).amount;

    await program.methods
      .claimFunds()
      .accounts({
        beneficiary: student.publicKey,
        grantAccount: grantPda,
        escrowVault: vaultPda,
        beneficiaryTokenAccount: studentTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([student])
      .rpc();

    const postStudentBalance = (
      await getAccount(provider.connection, studentTokenAccount)
    ).amount;
    const diff = Number(postStudentBalance) - Number(preStudentBalance);

    // Should include 1/3 of 6,000,000,000 (2,000,000,000) + streaming elapsed
    expect(diff).to.be.greaterThan(2_000_000_000);
  });

  it("6. Rejects malicious third-party attempt to claim student grant", async () => {
    try {
      await program.methods
        .claimFunds()
        .accounts({
          beneficiary: maliciousActor.publicKey,
          grantAccount: grantPda,
          escrowVault: vaultPda,
          beneficiaryTokenAccount: studentTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([maliciousActor])
        .rpc();
      expect.fail("Expected unauthorized claim failure");
    } catch (err: any) {
      expect(err.toString()).to.include("UnauthorizedReviewer");
    }
  });
});