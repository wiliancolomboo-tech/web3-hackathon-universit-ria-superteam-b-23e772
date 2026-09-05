use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("Bo1saFi1111111111111111111111111111111111111");

#[program]
pub mod bolsafi {
    use super::*;

    /// Initializes a programmatic endowment pool for a university department or grant funder.
    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        pool_id: u64,
        bump: u8,
    ) -> Result<()> {
        let pool = &mut ctx.accounts.pool;
        pool.authority = ctx.accounts.authority.key();
        pool.token_mint = ctx.accounts.token_mint.key();
        pool.vault = ctx.accounts.vault.key();
        pool.pool_id = pool_id;
        pool.total_deposited = 0;
        pool.total_allocated = 0;
        pool.total_claimed = 0;
        pool.bump = bump;
        pool.paused = false;

        emit!(PoolInitialized {
            pool: pool.key(),
            authority: pool.authority,
            token_mint: pool.token_mint,
            pool_id,
        });

        Ok(())
    }

    /// Deposits funds into the endowment pool escrow.
    pub fn deposit_funds(ctx: Context<DepositFunds>, amount: u64) -> Result<()> {
        require!(amount > 0, BolsaFiError::ZeroAmountDeposit);

        // Transfer funds from sponsor ATA to the Pool Escrow Vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.sponsor_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.sponsor.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        let pool = &mut ctx.accounts.pool;
        pool.total_deposited = pool
            .total_deposited
            .checked_add(amount)
            .ok_or(BolsaFiError::MathOverflow)?;

        emit!(FundsDeposited {
            pool: pool.key(),
            amount,
            new_total: pool.total_deposited,
        });

        Ok(())
    }

    /// Opens a continuous grant stream for a student researcher with an optional milestone bonus.
    pub fn create_stream_grant(
        ctx: Context<CreateStreamGrant>,
        rate_per_second: u64,
        cliff_seconds: i64,
        duration_seconds: i64,
        milestone_bonus: u64,
        bump: u8,
    ) -> Result<()> {
        require!(rate_per_second > 0, BolsaFiError::InvalidStreamingRate);
        require!(duration_seconds > 0, BolsaFiError::InvalidDuration);

        let clock = Clock::get()?;
        let current_time = clock.unix_timestamp;

        let total_linear_grant = rate_per_second
            .checked_mul(duration_seconds as u64)
            .ok_or(BolsaFiError::MathOverflow)?;
        let total_grant = total_linear_grant
            .checked_add(milestone_bonus)
            .ok_or(BolsaFiError::MathOverflow)?;

        let pool = &mut ctx.accounts.pool;
        let available = pool
            .total_deposited
            .checked_sub(pool.total_allocated)
            .ok_or(BolsaFiError::InsufficientPoolLiquidity)?;
        require!(available >= total_grant, BolsaFiError::InsufficientPoolLiquidity);

        pool.total_allocated = pool
            .total_allocated
            .checked_add(total_grant)
            .ok_or(BolsaFiError::MathOverflow)?;

        let grant = &mut ctx.accounts.grant;
        grant.pool = pool.key();
        grant.student = ctx.accounts.student.key();
        grant.advisor = ctx.accounts.advisor.key();
        grant.rate_per_second = rate_per_second;
        grant.start_time = current_time;
        grant.cliff_time = current_time
            .checked_add(cliff_seconds)
            .ok_or(BolsaFiError::MathOverflow)?;
        grant.end_time = current_time
            .checked_add(duration_seconds)
            .ok_or(BolsaFiError::MathOverflow)?;
        grant.last_claimed_timestamp = current_time;
        grant.total_funded = total_grant;
        grant.total_claimed = 0;
        grant.milestone_bonus = milestone_bonus;
        grant.milestone_unlocked = false;
        grant.is_paused = false;
        grant.bump = bump;

        emit!(GrantCreated {
            grant: grant.key(),
            student: grant.student,
            total_funded: total_grant,
            end_time: grant.end_time,
        });

        Ok(())
    }

    /// Verifies academic progress (e.g., Thesis Chapter approval, GitHub PR, research paper acceptance).
    pub fn attest_milestone(ctx: Context<AttestMilestone>) -> Result<()> {
        let grant = &mut ctx.accounts.grant;
        require!(!grant.milestone_unlocked, BolsaFiError::MilestoneAlreadyAttested);
        require!(!grant.is_paused, BolsaFiError::GrantPaused);

        grant.milestone_unlocked = true;

        emit!(MilestoneAttested {
            grant: grant.key(),
            advisor: ctx.accounts.advisor.key(),
            bonus_unlocked: grant.milestone_bonus,
        });

        Ok(())
    }

    /// Student claims available streamed tokens. Can be called at any second.
    pub fn claim_stream(ctx: Context<ClaimStream>) -> Result<()> {
        let grant = &mut ctx.accounts.grant;
        require!(!grant.is_paused, BolsaFiError::GrantPaused);

        let clock = Clock::get()?;
        let current_time = clock.unix_timestamp;

        require!(current_time >= grant.cliff_time, BolsaFiError::CliffNotReached);

        let calculation_time = current_time.min(grant.end_time);
        let active_duration = calculation_time
            .checked_sub(grant.last_claimed_timestamp)
            .ok_or(BolsaFiError::MathOverflow)?;

        if active_duration <= 0 && !grant.milestone_unlocked {
            return Err(BolsaFiError::NoClaimableTokens.into());
        }

        let mut claimable = (active_duration as u64)
            .checked_mul(grant.rate_per_second)
            .ok_or(BolsaFiError::MathOverflow)?;

        // If milestone has been validated by advisor and not yet paid out, add bonus
        if grant.milestone_unlocked && grant.milestone_bonus > 0 {
            claimable = claimable
                .checked_add(grant.milestone_bonus)
                .ok_or(BolsaFiError::MathOverflow)?;
            // Set milestone bonus to 0 to prevent double claiming
            grant.milestone_bonus = 0;
        }

        require!(claimable > 0, BolsaFiError::NoClaimableTokens);

        // Security assertion: cannot exceed total funded
        let projected_total = grant
            .total_claimed
            .checked_add(claimable)
            .ok_or(BolsaFiError::MathOverflow)?;
        require!(projected_total <= grant.total_funded, BolsaFiError::ExceedsAllocatedTotal);

        grant.last_claimed_timestamp = calculation_time;
        grant.total_claimed = projected_total;

        let pool = &mut ctx.accounts.pool;
        pool.total_claimed = pool
            .total_claimed
            .checked_add(claimable)
            .ok_or(BolsaFiError::MathOverflow)?;

        // Transfer funds using PDA seeds of the pool
        let pool_id_bytes = pool.pool_id.to_le_bytes();
        let authority_key = pool.authority.key();
        let seeds = &[
            b"endowment",
            authority_key.as_ref(),
            pool_id_bytes.as_ref(),
            &[pool.bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.student_token_account.to_account_info(),
            authority: pool.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);
        token::transfer(cpi_ctx, claimable)?;

        emit!(TokensClaimed {
            grant: grant.key(),
            student: grant.student,
            amount: claimable,
            total_claimed: grant.total_claimed,
        });

        Ok(())
    }

    /// Emergency pause or unpause in case of student academic withdrawal or institutional audit.
    pub fn toggle_pause_grant(ctx: Context<TogglePauseGrant>) -> Result<()> {
        let grant = &mut ctx.accounts.grant;
        grant.is_paused = !grant.is_paused;

        emit!(GrantPauseToggled {
            grant: grant.key(),
            is_paused: grant.is_paused,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION CONTEXTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(pool_id: u64, bump: u8)]
pub struct InitializePool<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + EndowmentPool::LEN,
        seeds = [b"endowment", authority.key().as_ref(), &pool_id.to_le_bytes()],
        bump
    )]
    pub pool: Account<'info, EndowmentPool>,

    pub token_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = authority,
        token::mint = token_mint,
        token::authority = pool,
        seeds = [b"vault", pool.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct DepositFunds<'info> {
    #[account(
        mut,
        has_one = vault,
        has_one = authority @ BolsaFiError::UnauthorizedSponsor
    )]
    pub pool: Account<'info, EndowmentPool>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub sponsor_token_account: Account<'info, TokenAccount>,

    pub authority: Signer<'info>,
    pub sponsor: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
#[instruction(rate_per_second: u64, cliff_seconds: i64, duration_seconds: i64, milestone_bonus: u64, bump: u8)]
pub struct CreateStreamGrant<'info> {
    #[account(
        mut,
        has_one = authority @ BolsaFiError::UnauthorizedSponsor
    )]
    pub pool: Account<'info, EndowmentPool>,

    #[account(
        init,
        payer = authority,
        space = 8 + GrantStream::LEN,
        seeds = [b"grant", pool.key().as_ref(), student.key().as_ref()],
        bump
    )]
    pub grant: Account<'info, GrantStream>,

    /// CHECK: Target student receiving academic stream
    pub student: AccountInfo<'info>,

    /// Academic advisor authorized to attest milestones
    /// CHECK: Verified via advisor parameter in GrantStream
    pub advisor: AccountInfo<'info>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AttestMilestone<'info> {
    #[account(
        mut,
        has_one = advisor @ BolsaFiError::UnauthorizedAdvisor
    )]
    pub grant: Account<'info, GrantStream>,
    pub advisor: Signer<'info>,
}

#[derive(Accounts)]
pub struct ClaimStream<'info> {
    #[account(
        mut,
        has_one = vault
    )]
    pub pool: Account<'info, EndowmentPool>,

    #[account(
        mut,
        seeds = [b"grant", pool.key().as_ref(), student.key().as_ref()],
        bump = grant.bump,
        has_one = pool @ BolsaFiError::MismatchedPool,
        has_one = student @ BolsaFiError::UnauthorizedStudent
    )]
    pub grant: Account<'info, GrantStream>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = student_token_account.owner == student.key() @ BolsaFiError::InvalidRecipientTokenAccount,
        constraint = student_token_account.mint == pool.token_mint @ BolsaFiError::InvalidMint
    )]
    pub student_token_account: Account<'info, TokenAccount>,

    pub student: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct TogglePauseGrant<'info> {
    #[account(
        has_one = authority @ BolsaFiError::UnauthorizedSponsor
    )]
    pub pool: Account<'info, EndowmentPool>,

    #[account(
        mut,
        has_one = pool @ BolsaFiError::MismatchedPool
    )]
    pub grant: Account<'info, GrantStream>,

    pub authority: Signer<'info>,
}

// -----------------------------------------------------------------------------
// STATE DEFINITIONS
// -----------------------------------------------------------------------------

#[account]
pub struct EndowmentPool {
    pub authority: Pubkey,       // 32
    pub token_mint: Pubkey,      // 32
    pub vault: Pubkey,           // 32
    pub pool_id: u64,            // 8
    pub total_deposited: u64,    // 8
    pub total_allocated: u64,    // 8
    pub total_claimed: u64,      // 8
    pub bump: u8,                // 1
    pub paused: bool,            // 1
}

impl EndowmentPool {
    pub const LEN: usize = 32 + 32 + 32 + 8 + 8 + 8 + 8 + 1 + 1;
}

#[account]
pub struct GrantStream {
    pub pool: Pubkey,                    // 32
    pub student: Pubkey,                 // 32
    pub advisor: Pubkey,                 // 32
    pub rate_per_second: u64,            // 8
    pub start_time: i64,                 // 8
    pub cliff_time: i64,                 // 8
    pub end_time: i64,                   // 8
    pub last_claimed_timestamp: i64,     // 8
    pub total_funded: u64,               // 8
    pub total_claimed: u64,              // 8
    pub milestone_bonus: u64,            // 8
    pub milestone_unlocked: bool,        // 1
    pub is_paused: bool,                 // 1
    pub bump: u8,                        // 1
}

impl GrantStream {
    pub const LEN: usize = 32 + 32 + 32 + 8 + 8 + 8 + 8 + 8 + 8 + 8 + 8 + 1 + 1 + 1;
}

// -----------------------------------------------------------------------------
// EVENTS
// -----------------------------------------------------------------------------

#[event]
pub struct PoolInitialized {
    pub pool: Pubkey,
    pub authority: Pubkey,
    pub token_mint: Pubkey,
    pub pool_id: u64,
}

#[event]
pub struct FundsDeposited {
    pub pool: Pubkey,
    pub amount: u64,
    pub new_total: u64,
}

#[event]
pub struct GrantCreated {
    pub grant: Pubkey,
    pub student: Pubkey,
    pub total_funded: u64,
    pub end_time: i64,
}

#[event]
pub struct MilestoneAttested {
    pub grant: Pubkey,
    pub advisor: Pubkey,
    pub bonus_unlocked: u64,
}

#[event]
pub struct TokensClaimed {
    pub grant: Pubkey,
    pub student: Pubkey,
    pub amount: u64,
    pub total_claimed: u64,
}

#[event]
pub struct GrantPauseToggled {
    pub grant: Pubkey,
    pub is_paused: bool,
}

// -----------------------------------------------------------------------------
// ERROR CODES
// -----------------------------------------------------------------------------

#[error_code]
pub enum BolsaFiError {
    #[msg("Deposit amount must be greater than zero")]
    ZeroAmountDeposit,
    #[msg("Arithmetic overflow occurred during balance calculation")]
    MathOverflow,
    #[msg("Grant stream rate per second must be strictly positive")]
    InvalidStreamingRate,
    #[msg("Stream duration must be positive")]
    InvalidDuration,
    #[msg("Endowment pool does not possess sufficient unallocated liquidity")]
    InsufficientPoolLiquidity,
    #[msg("Milestone has already been attested and unlocked")]
    MilestoneAlreadyAttested,
    #[msg("Grant stream is paused by faculty dean or sponsor")]
    GrantPaused,
    #[msg("Grant cliff period has not yet elapsed")]
    CliffNotReached,
    #[msg("No claimable tokens available at current timestamp")]
    NoClaimableTokens,
    #[msg("Calculated disbursement exceeds maximum total grant allocation")]
    ExceedsAllocatedTotal,
    #[msg("Signer is not authorized as the pool authority")]
    UnauthorizedSponsor,
    #[msg("Signer is not authorized as the registered academic advisor")]
    UnauthorizedAdvisor,
    #[msg("Provided pool account does not match grant state")]
    MismatchedPool,
    #[msg("Signer is not authorized as the beneficiary student")]
    UnauthorizedStudent,
    #[msg("Destination token account does not belong to beneficiary student")]
    InvalidRecipientTokenAccount,
    #[msg("Provided token account mint does not match endowment token mint")]
    InvalidMint,
}