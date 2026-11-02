use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("BoLSaFi1111111111111111111111111111111111111");

#[program]
pub mod bolsafi {
    use super::*;

    /// Initializes a scholarship grant pool for a university department or DAO sponsor.
    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        pool_id: u64,
        description: String,
    ) -> Result<()> {
        require!(description.as_bytes().len() <= 64, BolsaError::StringExceedsLimit);
        
        let pool = &mut ctx.accounts.pool;
        pool.sponsor = ctx.accounts.sponsor.key();
        pool.mint = ctx.accounts.mint.key();
        pool.pool_id = pool_id;
        pool.total_grants = 0;
        pool.total_allocated = 0;
        pool.bump = ctx.bumps.pool;

        emit!(PoolInitializedEvent {
            pool: pool.key(),
            sponsor: pool.sponsor,
            mint: pool.mint,
            pool_id,
        });

        Ok(())
    }

    /// Creates an algorithmic grant stream for a student researcher.
    pub fn create_stream(
        ctx: Context<CreateStream>,
        grant_id: u64,
        total_stream_amount: u64,
        milestone_amount: u64,
        start_time: i64,
        end_time: i64,
    ) -> Result<()> {
        let clock = Clock::get()?;
        let current_time = clock.unix_timestamp;

        require!(end_time > start_time, BolsaError::InvalidTimeWindow);
        require!(start_time >= current_time - 300, BolsaError::StartTimeTooFarInPast);
        require!(total_stream_amount > 0, BolsaError::ZeroAmount);

        let total_deposit = total_stream_amount
            .checked_add(milestone_amount)
            .ok_or(BolsaError::MathOverflow)?;

        // Execute CPI to transfer tokens from sponsor ATA to vault ATA
        let cpi_accounts = Transfer {
            from: ctx.accounts.sponsor_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.sponsor.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token::transfer(cpi_ctx, total_deposit)?;

        let grant = &mut ctx.accounts.grant;
        grant.pool = ctx.accounts.pool.key();
        grant.grant_id = grant_id;
        grant.student = ctx.accounts.student.key();
        grant.verifier = ctx.accounts.verifier.key();
        grant.total_stream_amount = total_stream_amount;
        grant.streamed_claimed = 0;
        grant.milestone_amount = milestone_amount;
        grant.milestone_claimed = false;
        grant.start_time = start_time;
        grant.end_time = end_time;
        grant.is_active = true;
        grant.bump = ctx.bumps.grant;
        grant.vault_bump = ctx.bumps.vault_authority;

        let pool = &mut ctx.accounts.pool;
        pool.total_grants = pool.total_grants.checked_add(1).ok_or(BolsaError::MathOverflow)?;
        pool.total_allocated = pool
            .total_allocated
            .checked_add(total_deposit)
            .ok_or(BolsaError::MathOverflow)?;

        emit!(StreamCreatedEvent {
            grant: grant.key(),
            pool: pool.key(),
            student: grant.student,
            total_amount: total_deposit,
            start_time,
            end_time,
        });

        Ok(())
    }

    /// Allows the student to claim accrued streaming tokens calculated to the second.
    pub fn claim_accrued(ctx: Context<ClaimAccrued>) -> Result<()> {
        let grant = &mut ctx.accounts.grant;
        require!(grant.is_active, BolsaError::GrantNotActive);

        let clock = Clock::get()?;
        let current_time = clock.unix_timestamp;

        require!(current_time > grant.start_time, BolsaError::StreamNotStarted);

        // Calculate accrued linear stream:
        // accrued = min(total_amount, total_amount * (current_time - start_time) / (end_time - start_time))
        let effective_time = std::cmp::min(current_time, grant.end_time);
        let elapsed_seconds = effective_time.saturating_sub(grant.start_time) as u128;
        let total_duration = (grant.end_time.saturating_sub(grant.start_time)) as u128;

        let total_accrued = ((grant.total_stream_amount as u128)
            .checked_mul(elapsed_seconds)
            .ok_or(BolsaError::MathOverflow)?
            .checked_div(total_duration)
            .ok_or(BolsaError::MathOverflow)?) as u64;

        let claimable = total_accrued.saturating_sub(grant.streamed_claimed);
        require!(claimable > 0, BolsaError::NoFundsAvailableToClaim);

        grant.streamed_claimed = grant
            .streamed_claimed
            .checked_add(claimable)
            .ok_or(BolsaError::MathOverflow)?;

        // Execute CPI signed by vault authority PDA
        let grant_key = grant.key();
        let seeds = &[
            b"authority",
            grant_key.as_ref(),
            &[grant.vault_bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.student_token_account.to_account_info(),
            authority: ctx.accounts.vault_authority.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token::transfer(cpi_ctx, claimable)?;

        emit!(AccruedClaimedEvent {
            grant: grant.key(),
            student: grant.student,
            amount_claimed: claimable,
            remaining_unclaimed: grant.total_stream_amount.saturating_sub(grant.streamed_claimed),
        });

        Ok(())
    }

    /// Verifier (e.g., Professor/Academic Director) approves milestone deliverables and releases tranche.
    pub fn approve_milestone(ctx: Context<ApproveMilestone>, milestone_notes: String) -> Result<()> {
        require!(milestone_notes.as_bytes().len() <= 128, BolsaError::StringExceedsLimit);
        let grant = &mut ctx.accounts.grant;
        require!(grant.is_active, BolsaError::GrantNotActive);
        require!(!grant.milestone_claimed, BolsaError::MilestoneAlreadyClaimed);
        require!(grant.milestone_amount > 0, BolsaError::NoMilestoneAllocated);

        grant.milestone_claimed = true;
        let amount = grant.milestone_amount;

        let grant_key = grant.key();
        let seeds = &[
            b"authority",
            grant_key.as_ref(),
            &[grant.vault_bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.student_token_account.to_account_info(),
            authority: ctx.accounts.vault_authority.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token::transfer(cpi_ctx, amount)?;

        emit!(MilestoneReleasedEvent {
            grant: grant.key(),
            student: grant.student,
            verifier: ctx.accounts.verifier.key(),
            amount,
        });

        Ok(())
    }

    /// Clawback unearned stream tokens + unapproved milestones back to Sponsor if research is abandoned.
    pub fn clawback_unvested(ctx: Context<ClawbackUnvested>) -> Result<()> {
        let grant = &mut ctx.accounts.grant;
        require!(grant.is_active, BolsaError::GrantNotActive);

        let clock = Clock::get()?;
        let current_time = clock.unix_timestamp;

        // Vested amount up to this point
        let effective_time = std::cmp::min(current_time, grant.end_time);
        let elapsed_seconds = effective_time.saturating_sub(grant.start_time) as u128;
        let total_duration = (grant.end_time.saturating_sub(grant.start_time)) as u128;

        let vested_amount = if current_time < grant.start_time {
            0u64
        } else {
            ((grant.total_stream_amount as u128)
                .checked_mul(elapsed_seconds)
                .ok_or(BolsaError::MathOverflow)?
                .checked_div(total_duration)
                .ok_or(BolsaError::MathOverflow)?) as u64
        };

        // Unearned stream remainder
        let unearned_stream = grant.total_stream_amount.saturating_sub(vested_amount);
        
        // Milestone tranche clawed back if not claimed
        let unearned_milestone = if !grant.milestone_claimed {
            grant.milestone_amount
        } else {
            0u64
        };

        let total_refund = unearned_stream
            .checked_add(unearned_milestone)
            .ok_or(BolsaError::MathOverflow)?;

        require!(total_refund > 0, BolsaError::NoFundsAvailableToClawback);

        // Deactivate grant
        grant.is_active = false;

        let grant_key = grant.key();
        let seeds = &[
            b"authority",
            grant_key.as_ref(),
            &[grant.vault_bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.sponsor_token_account.to_account_info(),
            authority: ctx.accounts.vault_authority.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token::transfer(cpi_ctx, total_refund)?;

        emit!(GrantClawedBackEvent {
            grant: grant.key(),
            sponsor: ctx.accounts.sponsor.key(),
            refunded_amount: total_refund,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION CONTEXTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(pool_id: u64)]
pub struct InitializePool<'info> {
    #[account(
        init,
        payer = sponsor,
        space = 8 + PoolAccount::INIT_SPACE,
        seeds = [b"pool", sponsor.key().as_ref(), &pool_id.to_le_bytes()],
        bump
    )]
    pub pool: Account<'info, PoolAccount>,

    pub mint: Account<'info, Mint>,

    #[account(mut)]
    pub sponsor: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(grant_id: u64)]
pub struct CreateStream<'info> {
    #[account(
        mut,
        has_one = sponsor @ BolsaError::UnauthorizedSponsor,
        has_one = mint @ BolsaError::MintMismatch,
    )]
    pub pool: Account<'info, PoolAccount>,

    #[account(
        init,
        payer = sponsor,
        space = 8 + StreamGrant::INIT_SPACE,
        seeds = [b"grant", pool.key().as_ref(), &grant_id.to_le_bytes()],
        bump
    )]
    pub grant: Account<'info, StreamGrant>,

    /// CHECK: Vault authority PDA serving as transfer signer
    #[account(
        seeds = [b"authority", grant.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(
        init,
        payer = sponsor,
        token::mint = mint,
        token::authority = vault_authority,
        seeds = [b"vault", grant.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,

    pub mint: Account<'info, Mint>,

    #[account(
        mut,
        constraint = sponsor_token_account.owner == sponsor.key() @ BolsaError::InvalidTokenOwner,
        constraint = sponsor_token_account.mint == mint.key() @ BolsaError::MintMismatch
    )]
    pub sponsor_token_account: Account<'info, TokenAccount>,

    /// CHECK: Recipient academic researcher
    pub student: UncheckedAccount<'info>,

    /// CHECK: Designated academic professor/evaluator
    pub verifier: UncheckedAccount<'info>,

    #[account(mut)]
    pub sponsor: Signer<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct ClaimAccrued<'info> {
    #[account(
        mut,
        has_one = student @ BolsaError::UnauthorizedStudent,
    )]
    pub grant: Account<'info, StreamGrant>,

    /// CHECK: Vault authority PDA
    #[account(
        seeds = [b"authority", grant.key().as_ref()],
        bump = grant.vault_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [b"vault", grant.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = student_token_account.owner == student.key() @ BolsaError::InvalidTokenOwner,
    )]
    pub student_token_account: Account<'info, TokenAccount>,

    pub student: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ApproveMilestone<'info> {
    #[account(
        mut,
        has_one = verifier @ BolsaError::UnauthorizedVerifier,
        has_one = student @ BolsaError::InvalidStudentAccount,
    )]
    pub grant: Account<'info, StreamGrant>,

    /// CHECK: Vault authority PDA
    #[account(
        seeds = [b"authority", grant.key().as_ref()],
        bump = grant.vault_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [b"vault", grant.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,

    /// CHECK: Student receiving tranche
    pub student: UncheckedAccount<'info>,

    #[account(
        mut,
        constraint = student_token_account.owner == grant.student @ BolsaError::InvalidTokenOwner,
    )]
    pub student_token_account: Account<'info, TokenAccount>,

    pub verifier: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ClawbackUnvested<'info> {
    #[account(
        mut,
        has_one = sponsor @ BolsaError::UnauthorizedSponsor,
    )]
    pub pool: Account<'info, PoolAccount>,

    #[account(
        mut,
        seeds = [b"grant", pool.key().as_ref(), &grant.grant_id.to_le_bytes()],
        bump = grant.bump,
        has_one = pool @ BolsaError::InvalidPoolAccount,
    )]
    pub grant: Account<'info, StreamGrant>,

    /// CHECK: Vault authority PDA
    #[account(
        seeds = [b"authority", grant.key().as_ref()],
        bump = grant.vault_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [b"vault", grant.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = sponsor_token_account.owner == sponsor.key() @ BolsaError::InvalidTokenOwner,
    )]
    pub sponsor_token_account: Account<'info, TokenAccount>,

    pub sponsor: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

// -----------------------------------------------------------------------------
// STATE STRUCTURES
// -----------------------------------------------------------------------------

#[account]
#[derive(InitSpace)]
pub struct PoolAccount {
    pub sponsor: Pubkey,         // 32
    pub mint: Pubkey,            // 32
    pub pool_id: u64,            // 8
    pub total_grants: u64,       // 8
    pub total_allocated: u64,    // 8
    pub bump: u8,                // 1
}

#[account]
#[derive(InitSpace)]
pub struct StreamGrant {
    pub pool: Pubkey,                // 32
    pub grant_id: u64,               // 8
    pub student: Pubkey,             // 32
    pub verifier: Pubkey,            // 32
    pub total_stream_amount: u64,    // 8
    pub streamed_claimed: u64,       // 8
    pub milestone_amount: u64,       // 8
    pub milestone_claimed: bool,     // 1
    pub start_time: i64,             // 8
    pub end_time: i64,               // 8
    pub is_active: bool,             // 1
    pub bump: u8,                    // 1
    pub vault_bump: u8,              // 1
}

// -----------------------------------------------------------------------------
// EVENTS
// -----------------------------------------------------------------------------

#[event]
pub struct PoolInitializedEvent {
    pub pool: Pubkey,
    pub sponsor: Pubkey,
    pub mint: Pubkey,
    pub pool_id: u64,
}

#[event]
pub struct StreamCreatedEvent {
    pub grant: Pubkey,
    pub pool: Pubkey,
    pub student: Pubkey,
    pub total_amount: u64,
    pub start_time: i64,
    pub end_time: i64,
}

#[event]
pub struct AccruedClaimedEvent {
    pub grant: Pubkey,
    pub student: Pubkey,
    pub amount_claimed: u64,
    pub remaining_unclaimed: u64,
}

#[event]
pub struct MilestoneReleasedEvent {
    pub grant: Pubkey,
    pub student: Pubkey,
    pub verifier: Pubkey,
    pub amount: u64,
}

#[event]
pub struct GrantClawedBackEvent {
    pub grant: Pubkey,
    pub sponsor: Pubkey,
    pub refunded_amount: u64,
}

// -----------------------------------------------------------------------------
// ERROR CODES
// -----------------------------------------------------------------------------

#[error_code]
pub enum BolsaError {
    #[msg("String length exceeds maximal permitted size.")]
    StringExceedsLimit,
    #[msg("Invalid time window: end time must precede start time.")]
    InvalidTimeWindow,
    #[msg("Start time cannot be set more than 300 seconds in the past.")]
    StartTimeTooFarInPast,
    #[msg("Grant stream amount must be greater than zero.")]
    ZeroAmount,
    #[msg("Mathematical calculation triggered an overflow.")]
    MathOverflow,
    #[msg("Grant stream is marked inactive or has already been clawed back.")]
    GrantNotActive,
    #[msg("Stream duration has not started yet.")]
    StreamNotStarted,
    #[msg("No unlocked streaming funds are available for claiming.")]
    NoFundsAvailableToClaim,
    #[msg("Milestone allocation has already been released.")]
    MilestoneAlreadyClaimed,
    #[msg("No milestone funds allocated for this grant.")]
    NoMilestoneAllocated,
    #[msg("No eligible unvested capital available to clawback.")]
    NoFundsAvailableToClawback,
    #[msg("Unauthorized: Signer does not match the sponsor on record.")]
    UnauthorizedSponsor,
    #[msg("Unauthorized: Signer does not match the student scholar.")]
    UnauthorizedStudent,
    #[msg("Unauthorized: Signer does not match the designated academic verifier.")]
    UnauthorizedVerifier,
    #[msg("Token account mint does not match grant requirement.")]
    MintMismatch,
    #[msg("Provided token account does not match expected authority/owner.")]
    InvalidTokenOwner,
    #[msg("Provided student account mismatches grant recipient record.")]
    InvalidStudentAccount,
    #[msg("Pool account provided does not match grant seed parameters.")]
    InvalidPoolAccount,
}