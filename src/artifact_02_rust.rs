use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("PactoBRL11111111111111111111111111111111111");

pub const STREAM_SEED: &[u8] = b"stream";
pub const VAULT_SEED: &[u8] = b"vault";
pub const ESCROW_SEED: &[u8] = b"pix_escrow";

#[program]
pub mod pactobrl {
    use super::*;

    /// Initializes a linear stipend grant stream from a grantor (funder) to a student.
    pub fn create_stream(
        ctx: Context<CreateStream>,
        stream_id: u64,
        total_amount: u64,
        duration_seconds: i64,
    ) -> Result<()> {
        require!(total_amount > 0, PactoError::InvalidDepositAmount);
        require!(duration_seconds > 0, PactoError::InvalidDuration);

        let clock = Clock::get()?;
        let start_time = clock.unix_timestamp;
        let end_time = start_time
            .checked_add(duration_seconds)
            .ok_or(PactoError::MathOverflow)?;

        let stream = &mut ctx.accounts.stream_account;
        stream.funder = ctx.accounts.funder.key();
        stream.recipient = ctx.accounts.recipient.key();
        stream.mint = ctx.accounts.mint.key();
        stream.stream_id = stream_id;
        stream.total_amount = total_amount;
        stream.claimed_amount = 0;
        stream.start_time = start_time;
        stream.end_time = end_time;
        stream.bump = ctx.bumps.stream_account;
        stream.vault_bump = ctx.bumps.vault_account;
        stream.is_paused = false;

        // Transfer funds from funder to PDA-controlled vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.funder_token_account.to_account_info(),
            to: ctx.accounts.vault_account.to_account_info(),
            authority: ctx.accounts.funder.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token::transfer(cpi_ctx, total_amount)?;

        emit!(StreamCreatedEvent {
            funder: stream.funder,
            recipient: stream.recipient,
            stream_id,
            total_amount,
            start_time,
            end_time,
        });

        Ok(())
    }

    /// Allows the student recipient to atomically withdraw whatever has vested linearly.
    pub fn withdraw_stream_accrual(ctx: Context<WithdrawStream>) -> Result<()> {
        let stream = &mut ctx.accounts.stream_account;
        let clock = Clock::get()?;

        require!(!stream.is_paused, PactoError::StreamIsPaused);

        let current_time = clock.unix_timestamp;
        let vested_amount = if current_time >= stream.end_time {
            stream.total_amount
        } else if current_time <= stream.start_time {
            0
        } else {
            let elapsed = current_time
                .checked_sub(stream.start_time)
                .ok_or(PactoError::MathOverflow)?;
            let total_duration = stream
                .end_time
                .checked_sub(stream.start_time)
                .ok_or(PactoError::MathOverflow)?;

            let accrued = (stream.total_amount as u128)
                .checked_mul(elapsed as u128)
                .ok_or(PactoError::MathOverflow)?
                .checked_div(total_duration as u128)
                .ok_or(PactoError::MathOverflow)? as u64;

            accrued
        };

        let claimable = vested_amount
            .checked_sub(stream.claimed_amount)
            .ok_or(PactoError::NothingToClaim)?;

        require!(claimable > 0, PactoError::NothingToClaim);

        stream.claimed_amount = stream
            .claimed_amount
            .checked_add(claimable)
            .ok_or(PactoError::MathOverflow)?;

        // Execute CPI from PDA vault authority
        let funder_key = stream.funder;
        let recipient_key = stream.recipient;
        let id_bytes = stream.stream_id.to_le_bytes();

        let vault_seeds: &[&[&[u8]]] = &[&[
            VAULT_SEED,
            stream.to_account_info().key.as_ref(),
            &[stream.vault_bump],
        ]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault_account.to_account_info(),
            to: ctx.accounts.recipient_token_account.to_account_info(),
            authority: ctx.accounts.vault_account.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            vault_seeds,
        );
        token::transfer(cpi_ctx, claimable)?;

        emit!(AccrualClaimedEvent {
            stream_id: stream.stream_id,
            recipient: recipient_key,
            amount: claimable,
            remaining_unvested: stream.total_amount.saturating_sub(stream.claimed_amount),
        });

        Ok(())
    }

    /// Initializes a P2P atomic off-ramp escrow backed by SPL tokens.
    pub fn open_pix_escrow(
        ctx: Context<OpenPixEscrow>,
        escrow_id: u64,
        amount: u64,
        pix_payment_hash: [u8; 32],
        timelock_slots: u64,
    ) -> Result<()> {
        require!(amount > 0, PactoError::InvalidDepositAmount);

        let escrow = &mut ctx.accounts.pix_escrow;
        escrow.maker = ctx.accounts.maker.key();
        escrow.student = ctx.accounts.student.key();
        escrow.mint = ctx.accounts.mint.key();
        escrow.escrow_id = escrow_id;
        escrow.amount = amount;
        escrow.pix_payment_hash = pix_payment_hash;
        escrow.expiry_slot = Clock::get()?
            .slot
            .checked_add(timelock_slots)
            .ok_or(PactoError::MathOverflow)?;
        escrow.status = EscrowStatus::AwaitingFiatTransfer;
        escrow.bump = ctx.bumps.pix_escrow;

        // Student deposits their claim into escrow
        let cpi_accounts = Transfer {
            from: ctx.accounts.student_token_account.to_account_info(),
            to: ctx.accounts.escrow_token_vault.to_account_info(),
            authority: ctx.accounts.student.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        Ok(())
    }

    /// Releases escrowed SPL tokens to the liquidity maker upon confirmed PIX End-to-End delivery.
    pub fn release_pix_escrow(
        ctx: Context<ReleasePixEscrow>,
        pix_preimage: [u8; 32],
    ) -> Result<()> {
        let escrow = &mut ctx.accounts.pix_escrow;
        require!(
            escrow.status == EscrowStatus::AwaitingFiatTransfer,
            PactoError::InvalidEscrowState
        );

        // Verify SHA-256 hash preimage of the PIX EndToEndId receipt
        let computed_hash = anchor_lang::solana_program::hash::hash(&pix_preimage);
        require!(
            computed_hash.to_bytes() == escrow.pix_payment_hash,
            PactoError::InvalidPreimageHash
        );

        escrow.status = EscrowStatus::Completed;

        let maker_key = escrow.maker;
        let student_key = escrow.student;
        let id_bytes = escrow.escrow_id.to_le_bytes();
        let bump_slice = &[escrow.bump];

        let escrow_seeds: &[&[&[u8]]] = &[&[
            ESCROW_SEED,
            maker_key.as_ref(),
            student_key.as_ref(),
            id_bytes.as_ref(),
            bump_slice,
        ]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.escrow_token_vault.to_account_info(),
            to: ctx.accounts.maker_token_account.to_account_info(),
            authority: ctx.accounts.pix_escrow.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            escrow_seeds,
        );
        token::transfer(cpi_ctx, escrow.amount)?;

        emit!(PixEscrowSettledEvent {
            escrow_id: escrow.escrow_id,
            maker: maker_key,
            student: student_key,
            amount: escrow.amount,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION CONTEXTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(stream_id: u64)]
pub摸struct CreateStream<'info> {
    #[account(mut)]
    pub funder: Signer<'info>,

    /// CHECK: Recipient is arbitrary Solana wallet owned by student
    pub recipient: UncheckedAccount<'info>,

    pub mint: Account<'info, Mint>,

    #[account(
        mut,
        constraint = funder_token_account.mint == mint.key(),
        constraint = funder_token_account.owner == funder.key()
    )]
    pub funder_token_account: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = funder,
        space = 8 + StreamAccount::LEN,
        seeds = [STREAM_SEED, funder.key().as_ref(), recipient.key().as_ref(), &stream_id.to_le_bytes()],
        bump
    )]
    pub stream_account: Account<'info, StreamAccount>,

    #[account(
        init,
        payer = funder,
        seeds = [VAULT_SEED, stream_account.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = vault_account
    )]
    pub vault_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct WithdrawStream<'info> {
    #[account(mut)]
    pub recipient: Signer<'info>,

    #[account(
        mut,
        has_one = recipient @ PactoError::UnauthorizedRecipient,
        has_one = vault_account @ PactoError::InvalidVaultAccount,
    )]
    pub stream_account: Account<'info, StreamAccount>,

    #[account(
        mut,
        seeds = [VAULT_SEED, stream_account.key().as_ref()],
        bump = stream_account.vault_bump
    )]
    pub vault_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = recipient_token_account.mint == stream_account.mint,
        constraint = recipient_token_account.owner == recipient.key()
    )]
    pub recipient_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
#[instruction(escrow_id: u64)]
pub struct OpenPixEscrow<'info> {
    #[account(mut)]
    pub student: Signer<'info>,

    /// CHECK: P2P Liquidity provider wallet
    pub maker: UncheckedAccount<'info>,

    pub mint: Account<'info, Mint>,

    #[account(
        mut,
        constraint = student_token_account.mint == mint.key(),
        constraint = student_token_account.owner == student.key()
    )]
    pub student_token_account: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = student,
        space = 8 + PixEscrowAccount::LEN,
        seeds = [ESCROW_SEED, maker.key().as_ref(), student.key().as_ref(), &escrow_id.to_le_bytes()],
        bump
    )]
    pub pix_escrow: Account<'info, PixEscrowAccount>,

    #[account(
        init,
        payer = student,
        token::mint = mint,
        token::authority = pix_escrow
    )]
    pub escrow_token_vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct ReleasePixEscrow<'info> {
    /// Can be confirmed by student upon seeing PIX arrival, or signed with oracle proof
    pub student: Signer<'info>,

    #[account(
        mut,
        has_one = student @ PactoError::UnauthorizedRecipient,
        seeds = [
            ESCROW_SEED,
            pix_escrow.maker.as_ref(),
            pix_escrow.student.as_ref(),
            &pix_escrow.escrow_id.to_le_bytes()
        ],
        bump = pix_escrow.bump
    )]
    pub pix_escrow: Account<'info, PixEscrowAccount>,

    #[account(
        mut,
        constraint = escrow_token_vault.owner == pix_escrow.key()
    )]
    pub escrow_token_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = maker_token_account.mint == pix_escrow.mint,
        constraint = maker_token_account.owner == pix_escrow.maker
    )]
    pub maker_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

// -----------------------------------------------------------------------------
// STATE SCHEMAS & DATA STRUCTURES
// -----------------------------------------------------------------------------

#[account]
pub struct StreamAccount {
    pub funder: Pubkey,          // 32
    pub recipient: Pubkey,       // 32
    pub mint: Pubkey,            // 32
    pub stream_id: u64,          // 8
    pub total_amount: u64,       // 8
    pub claimed_amount: u64,     // 8
    pub start_time: i64,         // 8
    pub end_time: i64,           // 8
    pub bump: u8,                // 1
    pub vault_bump: u8,          // 1
    pub is_paused: bool,         // 1
}

impl StreamAccount {
    pub const LEN: usize = 32 + 32 + 32 + 8 + 8 + 8 + 8 + 8 + 1 + 1 + 1 + 32; // Includes padding
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum EscrowStatus {
    AwaitingFiatTransfer,
    Completed,
    Refunded,
}

#[account]
pub struct PixEscrowAccount {
    pub maker: Pubkey,           // 32
    pub student: Pubkey,         // 32
    pub mint: Pubkey,            // 32
    pub escrow_id: u64,          // 8
    pub amount: u64,             // 8
    pub pix_payment_hash: [u8; 32], // 32
    pub expiry_slot: u64,        // 8
    pub status: EscrowStatus,    // 1
    pub bump: u8,                // 1
}

impl PixEscrowAccount {
    pub const LEN: usize = 32 + 32 + 32 + 8 + 8 + 32 + 8 + 1 + 1 + 32;
}

// -----------------------------------------------------------------------------
// EVENTS & ERRORS
// -----------------------------------------------------------------------------

#[event]
pub struct StreamCreatedEvent {
    pub funder: Pubkey,
    pub recipient: Pubkey,
    pub stream_id: u64,
    pub total_amount: u64,
    pub start_time: i64,
    pub end_time: i64,
}

#[event]
pub struct AccrualClaimedEvent {
    pub stream_id: u64,
    pub recipient: Pubkey,
    pub amount: u64,
    pub remaining_unvested: u64,
}

#[event]
pub struct PixEscrowSettledEvent {
    pub escrow_id: u64,
    pub maker: Pubkey,
    pub student: Pubkey,
    pub amount: u64,
}

#[error_code]
pub enum PactoError {
    #[msg("Deposit amount must be strictly greater than zero.")]
    InvalidDepositAmount,
    #[msg("Duration must be greater than zero seconds.")]
    InvalidDuration,
    #[msg("Arithmetic calculation resulted in an overflow/underflow.")]
    MathOverflow,
    #[msg("Stream is currently paused.")]
    StreamIsPaused,
    #[msg("Zero claimable tokens available at this timestamp.")]
    NothingToClaim,
    #[msg("Caller is not authorized to act as recipient.")]
    UnauthorizedRecipient,
    #[msg("Vault address provided does not match stream record.")]
    InvalidVaultAccount,
    #[msg("Provided preimage does not match registered PIX verification hash.")]
    InvalidPreimageHash,
    #[msg("Escrow is not in the required state.")]
    InvalidEscrowState,
}