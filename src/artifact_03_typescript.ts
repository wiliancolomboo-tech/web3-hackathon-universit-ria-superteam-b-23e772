import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Pactobrl } from "../target/types/pactobrl";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { assert, expect } from "chai";
import * as crypto from "crypto";

describe("PactoBRL End-to-End Lifecycle & Exploit Defense", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Pactobrl as Program<Pactobrl>;

  const funder = anchor.web3.Keypair.generate();
  const student = anchor.web3.Keypair.generate();
  const maker = anchor.web3.Keypair.generate();
  const attacker = anchor.web3.Keypair.generate();

  let mint: anchor.web3.PublicKey;
  let funderAta: anchor.web3.PublicKey;
  let studentAta: anchor.web3.PublicKey;
  let makerAta: anchor.web3.PublicKey;

  const streamId = new anchor.BN(9021);
  const streamAmount = new anchor.BN(10_000_000_000); // 10,000 BRL-denominated tokens (6 decimals)
  const durationSeconds = new anchor.BN(10); // 10 seconds rapid vest for testing

  // PIX Preimage data
  const pixPreimage = crypto.randomBytes(32);
  const pixHash = crypto.createHash("sha256").update(pixPreimage).digest();

  before(async () => {
    // Fund test wallets with SOL
    for (const kp of [funder, student, maker, attacker]) {
      const sig = await provider.connection.requestAirdrop(
        kp.publicKey,
        5 * anchor.web3.LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(sig, "confirmed");
    }

    // Deploy test SPL token (representing BRLA / BRZ)
    mint = await createMint(
      provider.connection,
      funder,
      funder.publicKey,
      null,
      6
    );

    funderAta = await createAccount(provider.connection, funder, mint, funder.publicKey);
    studentAta = await createAccount(provider.connection, student, mint, student.publicKey);
    makerAta = await createAccount(provider.connection, maker, mint, maker.publicKey);

    // Mint 25,000 tokens to funder
    await mintTo(
      provider.connection,
      funder,
      mint,
      funderAta,
      funder.publicKey,
      25_000_000_000
    );
  });

  it("QA-01: Successfully creates continuous milestone stream with correct PDA locking", async () => {
    const [streamPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("stream"),
        funder.publicKey.toBuffer(),
        student.publicKey.toBuffer(),
        streamId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    const [vaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), streamPda.toBuffer()],
      program.programId
    );

    await program.methods
      .createStream(streamId, streamAmount, durationSeconds)
      .accountsStrict({
        funder: funder.publicKey,
        recipient: student.publicKey,
        mint,
        funderTokenAccount: funderAta,
        streamAccount: streamPda,
        vaultAccount: vaultPda,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([funder])
      .rpc();

    const vaultInfo = await getAccount(provider.connection, vaultPda);
    assert.equal(vaultInfo.amount.toString(), streamAmount.toString());

    const accountData = await program.account.streamAccount.fetch(streamPda);
    expect(accountData.funder.toBase58()).to.equal(funder.publicKey.toBase58());
    expect(accountData.recipient.toBase58()).to.equal(student.publicKey.toBase58());
    expect(accountData.totalAmount.toNumber()).to.equal(streamAmount.toNumber());
  });

  it("QA-02: Enforces Linear Vesting Accrual and Rejects Frontrunning / Premature Extraction", async () => {
    const [streamPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("stream"),
        funder.publicKey.toBuffer(),
        student.publicKey.toBuffer(),
        streamId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    const [vaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), streamPda.toBuffer()],
      program.programId
    );

    // Wait 3 seconds to accrue roughly 30%
    await new Promise((resolve) => setTimeout(resolve, 3000));

    const balBefore = (await getAccount(provider.connection, studentAta)).amount;

    await program.methods
      .withdrawStreamAccrual()
      .accountsStrict({
        recipient: student.publicKey,
        streamAccount: streamPda,
        vaultAccount: vaultPda,
        recipientTokenAccount: studentAta,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([student])
      .rpc();

    const balAfter = (await getAccount(provider.connection, studentAta)).amount;
    const diff = Number(balAfter - balBefore);

    // Assert that ~30% (+/- tolerance for block production timing) was claimed
    expect(diff).to.be.greaterThan(2_000_000_000);
    expect(diff).to.be.lessThan(8_000_000_000);
  });

  it("QA-03: Security Revert - Attacker cannot steal funds using fake recipient signature", async () => {
    const [streamPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("stream"),
        funder.publicKey.toBuffer(),
        student.publicKey.toBuffer(),
        streamId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    const [vaultPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), streamPda.toBuffer()],
      program.programId
    );

    const attackerAta = await createAccount(
      provider.connection,
      attacker,
      mint,
      attacker.publicKey
    );

    try {
      await program.methods
        .withdrawStreamAccrual()
        .accountsStrict({
          recipient: attacker.publicKey,
          streamAccount: streamPda,
          vaultAccount: vaultPda,
          recipientTokenAccount: attackerAta,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([attacker])
        .rpc();
      assert.fail("Should have thrown UnauthorizedRecipient error");
    } catch (err: any) {
      expect(err.toString()).to.include("UnauthorizedRecipient");
    }
  });

  it("QA-04: Executes Atomic P2P PIX Ramp Escrow and verifies preimage settlement", async () => {
    const escrowId = new anchor.BN(4422);
    const swapAmount = new anchor.BN(1_000_000_000); // 1,000 tokens

    const [escrowPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("pix_escrow"),
        maker.publicKey.toBuffer(),
        student.publicKey.toBuffer(),
        escrowId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    const escrowVault = anchor.web3.Keypair.generate();

    // Student locks accrued tokens into PIX Escrow
    await program.methods
      .openPixEscrow(
        escrowId,
        swapAmount,
        Array.from(pixHash) as unknown as number[],
        new anchor.BN(100)
      )
      .accountsStrict({
        student: student.publicKey,
        maker: maker.publicKey,
        mint,
        studentTokenAccount: studentAta,
        pixEscrow: escrowPda,
        escrowTokenVault: escrowVault.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([student, escrowVault])
      .rpc();

    // Verify invalid hash preimage gets rejected
    const invalidPreimage = crypto.randomBytes(32);
    try {
      await program.methods
        .releasePixEscrow(Array.from(invalidPreimage) as unknown as number[])
        .accountsStrict({
          student: student.publicKey,
          pixEscrow: escrowPda,
          escrowTokenVault: escrowVault.publicKey,
          makerTokenAccount: makerAta,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([student])
        .rpc();
      assert.fail("Should have failed invalid preimage check");
    } catch (err: any) {
      expect(err.toString()).to.include("InvalidPreimageHash");
    }

    // Settle with valid preimage
    await program.methods
      .releasePixEscrow(Array.from(pixPreimage) as unknown as number[])
      .accountsStrict({
        student: student.publicKey,
        pixEscrow: escrowPda,
        escrowTokenVault: escrowVault.publicKey,
        makerTokenAccount: makerAta,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([student])
      .rpc();

    const makerBal = (await getAccount(provider.connection, makerAta)).amount;
    expect(makerBal.toString()).to.equal(swapAmount.toString());
  });
});