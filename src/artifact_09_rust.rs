use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};
use crate::state::*;
use crate::errors::BolsaError;

#[derive(Accounts)]
pub partial struct ClaimFunds<'info> {
    #[account(mut)]
    pub beneficiary: Signer<'info>,

    #[account(
        mut,
        seeds = [GRANT_SEED, grant_account.sponsor.as_ref(), &grant_account.grant_id.to_le_bytes()],
        bump = grant_account.bump,
        constraint = grant_account.beneficiary == beneficiary.key() @ BolsaError::UnauthorizedReviewer,
        constraint = grant_account.status == GrantStatus::Active @ BolsaError::GrantInactive
    )]
    pub grant_account: Account<'info, GrantAccount>,

    #[account(
        mut,
        seeds = [VAULT_SEED, grant_account.key().as_ref()],
        bump = grant_account.vault_bump,
        constraint = escrow_vault.key() == grant_account.vault @ BolsaError::InvalidMint
    )]
    pub escrow_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = beneficiary_token_account.owner == beneficiary.key() @ BolsaError::UnauthorizedReviewer,
        constraint = beneficiary_token_account.mint == grant_account.mint @ BolsaError::InvalidMint
    )]
    pub beneficiary_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

pub fn handle(ctx: Context<ClaimFunds>) -> Result<()> {
    let grant = &mut ctx.accounts.grant_account;
    let current_timestamp = Clock::get()?.unix_timestamp;

    // 1. Calculate unlocked streaming amount
    let unlocked_stream = if current_timestamp <= grant.start_time {
        0u64
    } else if current_timestamp >= grant.end_time {
        grant.stream_amount
    } else {
        let elapsed = (current_timestamp - grant.start_time) as u128;
        let total_duration = (grant.end_time - grant.start_time) as u128;

        (grant.stream_amount as u128)
            .checked_mul(elapsed)
            .ok_or(BolsaError::MathOverflow)?
            .checked_div(total_duration)
            .ok_or(BolsaError::MathOverflow)? as u64
    };

    // 2. Calculate approved milestone tranche amount
    let mut approved_count: u64 = 0;
    for i in 0..(grant.total_milestones as usize) {
        if grant.milestones_approved[i] {
            approved_count = approved_count.checked_add(1).ok_or(BolsaError::MathOverflow)?;
        }
    }

    let unlocked_milestones = if grant.total_milestones > 0 {
        (grant.milestone_amount as u128)
            .checked_mul(approved_count as u128)
            .ok_or(BolsaError::MathOverflow)?
            .checked_div(grant.total_milestones as u128)
            .ok_or(BolsaError::MathOverflow)? as u64
    } else {
        0u64
    };

    // 3. Compute net entitlement
    let total_unlocked = unlocked_stream
        .checked_add(unlocked_milestones)
        .ok_or(BolsaError::MathOverflow)?;

    let claimable = total_unlocked
        .checked_sub(grant.claimed_amount)
        .ok_or(BolsaError::MathOverflow)?;

    require!(claimable > 0, BolsaError::NoAvailableFunds);

    // Update internal state
    grant.claimed_amount = grant
        .claimed_amount
        .checked_add(claimable)
        .ok_or(BolsaError::MathOverflow)?;

    if grant.claimed_amount >= grant.total_amount {
        grant.status = GrantStatus::Completed;
    }

    // PDA seeds for signing the vault transfer
    let sponsor_key = grant.sponsor;
    let grant_id_bytes = grant.grant_id.to_le_bytes();
    let signer_seeds: &[&[&[u8]]] = &[&[
        GRANT_SEED,
        sponsor_key.as_ref(),
        &grant_id_bytes,
        &[grant.bump],
    ]];

    // Execute transfer via CPI
    let cpi_accounts = Transfer {
        from: ctx.accounts.escrow_vault.to_account_info(),
        to: ctx.accounts.beneficiary_token_account.to_account_info(),
        authority: grant.to_account_info(),
    };
    let cpi_ctx = CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info(),
        cpi_accounts,
        signer_seeds,
    );
    token::transfer(cpi_ctx, claimable)?;

    msg!(
        "Claim successful: {} tokens disbursed to {}. Total claimed: {}/{}",
        claimable,
        ctx.accounts.beneficiary.key(),
        grant.claimed_amount,
        grant.total_amount
    );

    Ok(())
}