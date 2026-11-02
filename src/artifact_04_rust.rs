use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("BoLSa11111111111111111111111111111111111111");

#[program]
pub mod bolsastream {
    use super::*;

    /// Initialize a new academic grant with stream rates and attestation criteria.
    pub fn initialize_grant(
        ctx: Context<InitializeGrant>,
        grant_id: u64,
        total_allocation: u64,
        start_time: i64,
        end_time: i64,
        required_attestations: u8,
    ) -> Result<()> {
        require!(start_time < end_time, GrantError::InvalidTimeRange);
        require!(total_allocation > 0, GrantError::ZeroAllocation);
        require!(required_attestations > 0, GrantError::InvalidAttestationThreshold);

        let grant = &mut ctx.accounts.grant;
        grant.grantor = ctx.accounts.grantor.key();
        grant.beneficiary = ctx.accounts.beneficiary.key();
        grant.mint = ctx.accounts.mint.key();
        grant.vault = ctx.accounts.vault.key();
        grant.grant_id = grant_id;
        grant.total_allocation = total_allocation;
        grant.claimed_amount = 0;
        grant.start_time = start_time;
        grant.end_time = end_time;
        grant.required_attestations = required_attestations;
        grant.is_cancelled = false;
        grant.vault_auth_bump = ctx.bumps.vault_authority;
        grant.grant_bump = ctx.bumps.grant;

        emit!(GrantInitializedEvent {
            grant_id,
            grantor: grant.grantor,
            beneficiary: grant.beneficiary,
            total_allocation,
            start_time,
            end_time,
        });

        Ok(())
    }

    /// Register a deterministic milestone for the grant.
    pub fn register_milestone(
        ctx: Context<RegisterMilestone>,
        milestone_index: u8,
        allocated_amount: u64,
        metadata_hash: [u8; 32],
    ) -> Result<()> {
        let milestone = &mut ctx.accounts.milestone;
        milestone.grant = ctx.accounts.grant.key();
        milestone.index = milestone_index;
        milestone.allocated_amount = allocated_amount;
        milestone.metadata_hash = metadata_hash;
        milestone.attestations_count = 0;
        milestone.is_completed = false;
        milestone.bump = ctx.bumps.milestone;

        emit!(MilestoneRegisteredEvent {
            grant: milestone.grant,
            milestone_index,
            allocated_amount,
            metadata_hash,
        });

        Ok(())
    }

    /// Attest to the completion of a milestone by an authorized auditor or advisor.
    pub fn attest_milestone(ctx: Context<AttestMilestone>, _milestone_index: u8) -> Result<()> {
        let milestone = &mut ctx.accounts.milestone;
        let grant = &ctx.accounts.grant;

        require!(!milestone.is_completed, GrantError::MilestoneAlreadyCompleted);
        require!(!grant.is_cancelled, GrantError::GrantCancelled);

        // Increment attestation count
        milestone.attestations_count = milestone
            .attestations_count
            .checked_add(1)
            .ok_or(GrantError::MathOverflow)?;

        emit!(MilestoneAttestedEvent {
            grant: grant.key(),
            milestone: milestone.key(),
            attestor: ctx.accounts.attestor.key(),
            current_attestations: milestone.attestations_count,
        });

        if milestone.attestations_count >= grant.required_attestations {
            milestone.is_completed = true;
            emit!(MilestoneCompletedEvent {
                grant: grant.key(),
                milestone_index: milestone.index,
                allocated_amount: milestone.allocated_amount,
            });
        }

        Ok(())
    }

    /// Beneficiary claims accrued stream allocation based on time and completed milestones.
    pub fn claim_streamed_funds(ctx: Context<ClaimFunds>) -> Result<()> {
        let grant = &mut ctx.accounts.grant;
        let current_time = Clock::get()?.unix_timestamp;

        require!(!grant.is_cancelled, GrantError::GrantCancelled);
        require!(current_time >= grant.start_time, GrantError::StreamNotStarted);

        // Linear vesting calculation:
        // Available = (Total * min(now - start, duration)) / duration
        let duration = grant.end_time.saturating_sub(grant.start_time) as u128;
        let elapsed = if current_time >= grant.end_time {
            duration
        } else {
            (current_time.saturating_sub(grant.start_time)) as u128
        };

        let vested_linear = (grant.total_allocation as u128)
            .checked_mul(elapsed)
            .ok_or(GrantError::MathOverflow)?
            .checked_div(duration)
            .ok_or(GrantError::MathOverflow)? as u64;

        let claimable = vested_linear
            .checked_sub(grant.claimed_amount)
            .ok_or(GrantError::NoClaimableBalance)?;

        require!(claimable > 0, GrantError::NoClaimableBalance);

        // Update state prior to CPI invocation to prevent reentrancy
        grant.claimed_amount = grant
            .claimed_amount
            .checked_add(claimable)
            .ok_or(GrantError::MathOverflow)?;

        // Execute CPI transfer from PDA vault to beneficiary ATA
        let grant_key = grant.key();
        let seeds = &[
            b"vault_auth".as_ref(),
            grant_key.as_ref(),
            &[grant.vault_auth_bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.beneficiary_token_account.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            },
            signer_seeds,
        );

        token::transfer(cpi_ctx, claimable)?;

        emit!(FundsClaimedEvent {
            grant: grant.key(),
            beneficiary: grant.beneficiary,
            amount: claimable,
            total_claimed: grant.claimed_amount,
        });

        Ok(())
    }

    /// Grantor can cancel the grant in case of academic misconduct or cancellation.
    /// Unvested funds return to grantor, while earned vested funds remain claimable.
    pub fn cancel_and_clawback(ctx: Context<CancelAndClawback>) -> Result<()> {
        let grant = &mut ctx.accounts.grant;
        let current_time = Clock::get()?.unix_timestamp;

        require!(!grant.is_cancelled, GrantError::GrantAlreadyCancelled);

        let duration = grant.end_time.saturating_sub(grant.start_time) as u128;
        let elapsed = if current_time <= grant.start_time {
            0
        } else if current_time >= grant.end_time {
            duration
        } else {
            (current_time.saturating_sub(grant.start_time)) as u128
        };

        let vested = (grant.total_allocation as u128)
            .checked_mul(elapsed)
            .ok_or(GrantError::MathOverflow)?
            .checked_div(duration)
            .ok_or(GrantError::MathOverflow)? as u64;

        let unvested = grant
            .total_allocation
            .checked_sub(vested)
            .ok_or(GrantError::MathOverflow)?;

        grant.is_cancelled = true;

        if unvested > 0 {
            let grant_key = grant.key();
            let seeds = &[
                b"vault_auth".as_ref(),
                grant_key.as_ref(),
                &[grant.vault_auth_bump],
            ];
            let signer_seeds = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.vault.to_account_info(),
                    to: ctx.accounts.grantor_token_account.to_account_info(),
                    authority: ctx.accounts.vault_authority.to_account_info(),
                },
                signer_seeds,
            );

            token::transfer(cpi_ctx, unvested)?;
        }

        emit!(GrantCancelledEvent {
            grant: grant.key(),
            clawed_back_amount: unvested,
            timestamp: current_time,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION STRUCTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(grant_id: u64)]
pub struct InitializeGrant<'info> {
    #[account(mut)]
    pub grantor: Signer<'info>,

    /// CHECK: Beneficiary is the designated academic/student account
    pub beneficiary: AccountInfo<'info>,

    pub mint: Account<'info, Mint>,

    #[account(
        init,
        payer = grantor,
        space = 8 + Grant::LEN,
        seeds = [b"grant", grantor.key().as_ref(), &grant_id.to_le_bytes()],
        bump
    )]
    pub grant: Account<'info, Grant>,

    /// CHECK: PDA signer for vault custody
    #[account(
        seeds = [b"vault_auth", grant.key().as_ref()],
        bump
    )]
    pub vault_authority: AccountInfo<'info>,

    #[account(
        init,
        payer = grantor,
        token::mint = mint,
        token::authority = vault_authority,
    )]
    pub vault: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
#[instruction(milestone_index: u8)]
pub struct RegisterMilestone<'info> {
    #[account(
        mut,
        has_one = grantor @ GrantError::UnauthorizedGrantor
    )]
    pub grant: Account<'info, Grant>,

    #[account(mut)]
    pub grantor: Signer<'info>,

    #[account(
        init,
        payer = grantor,
        space = 8 + Milestone::LEN,
        seeds = [b"milestone", grant.key().as_ref(), &[milestone_index]],
        bump
    )]
    pub milestone: Account<'info, Milestone>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(milestone_index: u8)]
pub struct AttestMilestone<'info> {
    #[account(mut)]
    pub attestor: Signer<'info>,

    #[account(mut)]
    pub grant: Account<'info, Grant>,

    #[account(
        mut,
        seeds = [b"milestone", grant.key().as_ref(), &[milestone_index]],
        bump = milestone.bump,
        has_one = grant @ GrantError::GrantMismatch
    )]
    pub milestone: Account<'info, Milestone>,
}

#[derive(Accounts)]
pub struct ClaimFunds<'info> {
    #[account(mut)]
    pub beneficiary: Signer<'info>,

    #[account(
        mut,
        has_one = beneficiary @ GrantError::UnauthorizedBeneficiary,
        has_one = vault @ GrantError::InvalidVault
    )]
    pub grant: Account<'info, Grant>,

    /// CHECK: Vault authority PDA
    #[account(
        seeds = [b"vault_auth", grant.key().as_ref()],
        bump = grant.vault_auth_bump
    )]
    pub vault_authority: AccountInfo<'info>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = beneficiary_token_account.owner == beneficiary.key() @ GrantError::InvalidTokenAccountOwner,
        constraint = beneficiary_token_account.mint == grant.mint @ GrantError::InvalidMint
    )]
    pub beneficiary_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub clock: Sysvar<'info, Clock>,
}

#[derive(Accounts)]
pub struct CancelAndClawback<'info> {
    #[account(mut)]
    pub grantor: Signer<'info>,

    #[account(
        mut,
        has_one = grantor @ GrantError::UnauthorizedGrantor,
        has_one = vault @ GrantError::InvalidVault
    )]
    pub grant: Account<'info, Grant>,

    /// CHECK: Vault authority PDA
    #[account(
        seeds = [b"vault_auth", grant.key().as_ref()],
        bump = grant.vault_auth_bump
    )]
    pub vault_authority: AccountInfo<'info>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = grantor_token_account.owner == grantor.key() @ GrantError::InvalidTokenAccountOwner,
        constraint = grantor_token_account.mint == grant.mint @ GrantError::InvalidMint
    )]
    pub grantor_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub clock: Sysvar<'info, Clock>,
}

// -----------------------------------------------------------------------------
// STATE DEFINITIONS
// -----------------------------------------------------------------------------

#[account]
pub struct Grant {
    pub grantor: Pubkey,               // 32
    pub beneficiary: Pubkey,           // 32
    pub mint: Pubkey,                  // 32
    pub vault: Pubkey,                 // 32
    pub grant_id: u64,                 // 8
    pub total_allocation: u64,         // 8
    pub claimed_amount: u64,           // 8
    pub start_time: i64,               // 8
    pub end_time: i64,                 // 8
    pub required_attestations: u8,     // 1
    pub is_cancelled: bool,            // 1
    pub vault_auth_bump: u8,           // 1
    pub grant_bump: u8,                // 1
}

impl Grant {
    pub const LEN: usize = 32 + 32 + 32 + 32 + 8 + 8 + 8 + 8 + 8 + 1 + 1 + 1 + 1;
}

#[account]
pub struct Milestone {
    pub grant: Pubkey,                 // 32
    pub index: u8,                     // 1
    pub allocated_amount: u64,         // 8
    pub metadata_hash: [u8; 32],       // 32 (SHA-256 / IPFS CID of paper or deliverable)
    pub attestations_count: u8,        // 1
    pub is_completed: bool,            // 1
    pub bump: u8,                      // 1
}

impl Milestone {
    pub const LEN: usize = 32 + 1 + 8 + 32 + 1 + 1 + 1;
}

// -----------------------------------------------------------------------------
// EVENTS
// -----------------------------------------------------------------------------

#[event]
pub struct GrantInitializedEvent {
    pub grant_id: u64,
    pub grantor: Pubkey,
    pub beneficiary: Pubkey,
    pub total_allocation: u64,
    pub start_time: i64,
    pub end_time: i64,
}

#[event]
pub struct MilestoneRegisteredEvent {
    pub grant: Pubkey,
    pub milestone_index: u8,
    pub allocated_amount: u64,
    pub metadata_hash: [u8; 32],
}

#[event]
pub struct MilestoneAttestedEvent {
    pub grant: Pubkey,
    pub milestone: Pubkey,
    pub attestor: Pubkey,
    pub current_attestations: u8,
}

#[event]
pub struct MilestoneCompletedEvent {
    pub grant: Pubkey,
    pub milestone_index: u8,
    pub allocated_amount: u64,
}

#[event]
pub struct FundsClaimedEvent {
    pub grant: Pubkey,
    pub beneficiary: Pubkey,
    pub amount: u64,
    pub total_claimed: u64,
}

#[event]
pub struct GrantCancelledEvent {
    pub grant: Pubkey,
    pub clawed_back_amount: u64,
    pub timestamp: i64,
}

// -----------------------------------------------------------------------------
// ERROR CODES
// -----------------------------------------------------------------------------

#[error_code]
pub enum GrantError {
    #[msg("Invalid time range: start_time must be before end_time.")]
    InvalidTimeRange,
    #[msg("Total allocation must be greater than zero.")]
    ZeroAllocation,
    #[msg("Required attestations threshold must be at least 1.")]
    InvalidAttestationThreshold,
    #[msg("Milestone has already been attested and completed.")]
    MilestoneAlreadyCompleted,
    #[msg("The grant has been cancelled.")]
    GrantCancelled,
    #[msg("The grant has already been cancelled.")]
    GrantAlreadyCancelled,
    #[msg("Arithmetic overflow occurred.")]
    MathOverflow,
    #[msg("Stream has not started yet.")]
    StreamNotStarted,
    #[msg("No unlocked funds currently available to claim.")]
    NoClaimableBalance,
    #[msg("Caller is not the authorized grantor.")]
    UnauthorizedGrantor,
    #[msg("Caller is not the designated beneficiary.")]
    UnauthorizedBeneficiary,
    #[msg("Invalid vault account supplied.")]
    InvalidVault,
    #[msg("Token account does not match expected mint.")]
    InvalidMint,
    #[msg("Token account does not match expected owner.")]
    InvalidTokenAccountOwner,
    #[msg("Milestone does not belong to specified grant.")]
    GrantMismatch,
}