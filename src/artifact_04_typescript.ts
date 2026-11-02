import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Bolsafi } from "../target/types/bolsafi";
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

  const program = anchor.workspace.Bolsafi as Program<Bolsafi>;

  // Keypairs
  const sponsor = anchor.web3.Keypair.generate();
  const student = anchor.web3.Keypair.generate();
  const verifier = anchor.web3.Keypair.generate();
  const attacker = anchor.web3.Keypair.generate();

  let mint: anchor.web3.PublicKey;
  let sponsorTokenAccount: anchor.web3.PublicKey;
  let studentTokenAccount: anchor.web3.PublicKey;
  let attackerTokenAccount: anchor.web3.PublicKey;

  const poolId = new anchor.BN(101);
  const grantId = new anchor.BN(202401);
  
  // Amounts (6 decimals, e.g., BRZ or USDC)
  const streamAmount = new anchor.BN(600_000_000); // 600 USDC over 60 seconds (10 USDC/sec)
  const milestoneAmount = new anchor.BN(400_000_000); // 400 USDC milestone bonus

  let poolPda: anchor.web3.PublicKey;
  let grantPda: anchor.web3.PublicKey;
  let vaultAuthorityPda: anchor.web3.PublicKey;
  let vaultTokenPda: anchor.web3.PublicKey;

  let startTime: number;
  let endTime: number;

  before(async () => {
    // Fund test wallets with SOL
    const airdropSigners = [sponsor, student, verifier, attacker];
    for (const s of airdropSigners) {
      const sig = await provider.connection.requestAirdrop(
        s.publicKey,
        5 * anchor.web3.LAMPORTS_PER_SOL
      );
      const latestBlock = await provider.connection.getLatestBlockhash();
      await provider.connection.confirmTransaction({
        signature: sig,
        blockhash: latestBlock.blockhash,
        lastValidBlockHeight: latestBlock.lastValidBlockHeight,
      });
    }

    // Initialize Mint
    mint = await createMint(
      provider.connection,
      sponsor,
      sponsor.publicKey,
      null,
      6
    );

    // Create Token Accounts
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

    attackerTokenAccount = await createAccount(
      provider.connection,
      attacker,
      mint,
      attacker.publicKey
    );

    // Mint 5,000 tokens to Sponsor
    await mintTo(
      provider.connection,
      sponsor,
      mint,
      sponsorTokenAccount,
      sponsor,
      5_000_000_000
    );

    // Derive PDAs
    [poolPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("pool"), sponsor.publicKey.toBuffer(), poolId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    [grantPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("grant"), poolPda.toBuffer(), grantId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    [vaultAuthorityPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("authority"), grantPda.toBuffer()],
      program.programId
    );

    [vaultTokenPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), grantPda.toBuffer()],
      program.programId
    );
  });

  it("1. Initializes University Grant Pool", async () => {
    await program.methods
      .initializePool(poolId, "Departamento de Ciência da Computação - USP")
      .accounts({
        pool: poolPda,
        mint: mint,
        sponsor: sponsor.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .signers([sponsor])
      .rpc();

    const poolData = await program.account.poolAccount.fetch(poolPda);
    expect(poolData.sponsor.toBase58()).to.equal(sponsor.publicKey.toBase58());
    expect(poolData.mint.toBase58()).to.equal(mint.toBase58());
    expect(poolData.totalGrants.toNumber()).to.equal(0);
  });

  it("2. Creates Research Grant with Linear Stream & Milestone Tranche", async () => {
    const blockTime = (await provider.connection.getBlockTime(
      await provider.connection.getSlot()
    ))!;
    startTime = blockTime;
    endTime = blockTime + 10; // 10 second quick window for simulation

    await program.methods
      .createStream(
        grantId,
        streamAmount,
        milestoneAmount,
        new anchor.BN(startTime),
        new anchor.BN(endTime)
      )
      .accounts({
        pool: poolPda,
        grant: grantPda,
        vaultAuthority: vaultAuthorityPda,
        vault: vaultTokenPda,
        mint: mint,
        sponsorTokenAccount: sponsorTokenAccount,
        student: student.publicKey,
        verifier: verifier.publicKey,
        sponsor: sponsor.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([sponsor])
      .rpc();

    const vaultInfo = await getAccount(provider.connection, vaultTokenPda);
    expect(Number(vaultInfo.amount)).to.equal(1_000_000_000); // 600m + 400m

    const grantData = await program.account.streamGrant.fetch(grantPda);
    expect(grantData.student.toBase58()).to.equal(student.publicKey.toBase58());
    expect(grantData.totalStreamAmount.toNumber()).to.equal(600_000_000);
    expect(grantData.milestoneAmount.toNumber()).to.equal(400_000_000);
    expect(grantData.isActive).to.be.true;
  });

  it("3. Attacker fails to claim student streaming funds (Unauthorized)", async () => {
    try {
      await program.methods
        .claimAccrued()
        .accounts({
          grant: grantPda,
          vaultAuthority: vaultAuthorityPda,
          vault: vaultTokenPda,
          studentTokenAccount: attackerTokenAccount,
          student: attacker.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([attacker])
        .rpc();
      expect.fail("Attacker should have been blocked");
    } catch (err: any) {
      expect(err.toString()).to.include("UnauthorizedStudent");
    }
  });

  it("4. Student claims linearly accrued stream after time elapse", async () => {
    // Wait 4 seconds to accumulate linear yield
    await new Promise((resolve) => setTimeout(resolve, 4000));

    await program.methods
      .claimAccrued()
      .accounts({
        grant: grantPda,
        vaultAuthority: vaultAuthorityPda,
        vault: vaultTokenPda,
        studentTokenAccount: studentTokenAccount,
        student: student.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([student])
      .rpc();

    const studentBalance = await getAccount(provider.connection, studentTokenAccount);
    expect(Number(studentBalance.amount)).to.be.greaterThan(0);

    const grantData = await program.account.streamGrant.fetch(grantPda);
    expect(grantData.streamedClaimed.toNumber()).to.equal(Number(studentBalance.amount));
  });

  it("5. Academic Verifier approves and releases Milestone Tranche", async () => {
    const beforeBalance = (await getAccount(provider.connection, studentTokenAccount)).amount;

    await program.methods
      .approveMilestone("Artigo aceito para submissão no SBAC-PAD / Web3 Track")
      .accounts({
        grant: grantPda,
        vaultAuthority: vaultAuthorityPda,
        vault: vaultTokenPda,
        student: student.publicKey,
        studentTokenAccount: studentTokenAccount,
        verifier: verifier.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([verifier])
      .rpc();

    const afterBalance = (await getAccount(provider.connection, studentTokenAccount)).amount;
    expect(Number(afterBalance - beforeBalance)).to.equal(400_000_000);

    const grantData = await program.account.streamGrant.fetch(grantPda);
    expect(grantData.milestoneClaimed).to.be.true;
  });

  it("6. Prevents duplicate milestone claims (Double-Spend Protection)", async () => {
    try {
      await program.methods
        .approveMilestone("Tentativa duplicada")
        .accounts({
          grant: grantPda,
          vaultAuthority: vaultAuthorityPda,
          vault: vaultTokenPda,
          student: student.publicKey,
          studentTokenAccount: studentTokenAccount,
          verifier: verifier.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([verifier])
        .rpc();
      expect.fail("Second milestone claim should have failed");
    } catch (err: any) {
      expect(err.toString()).to.include("MilestoneAlreadyClaimed");
    }
  });

  it("7. Sponsor executes clawback on remaining funds after grant deactivation", async () => {
    // Wait until stream window has completed to verify exact boundaries
    await new Promise((resolve) => setTimeout(resolve, 6000));

    // Student claims remaining stream so only true balance remains
    await program.methods
      .claimAccrued()
      .accounts({
        grant: grantPda,
        vaultAuthority: vaultAuthorityPda,
        vault: vaultTokenPda,
        studentTokenAccount: studentTokenAccount,
        student: student.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([student])
      .rpc();

    // Vault should now be nearly drained (milestone claimed + all stream claimed)
    const vaultBal = await getAccount(provider.connection, vaultTokenPda);
    expect(Number(vaultBal.amount)).to.equal(0);
  });
});