use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use crate::state::*;
use crate::errors::BolsaError;

#[derive(Accounts)]
#[instruction(grant_id: u64)]
pub partial struct InitializeGrant<'info> {
    #[account(mut)]
    pub sponsor: Signer<'info>,

    /// CHECK: Beneficiary is the student wallet receiving streaming stipends
    pub beneficiary: UncheckedAccount<'info>,

    /// CHECK: Advisor is the validator who reviews research deliverables
    pub advisor: UncheckedAccount<'info>,

    pub mint: Account<'info, Mint>,

    #[account(
        init,
        payer = sponsor,
        space = GrantAccount::LEN,
        seeds = [GRANT_SEED, sponsor.key().as_ref(), &grant_id.to_le_bytes()],
        bump
    )]
    pub grant_account: Account<'info, GrantAccount>,

    #[account(
        init,
        payer = sponsor,
        seeds = [VAULT_SEED, grant_account.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = grant_account
    )]
    pub escrow_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = sponsor_token_account.owner == sponsor.key() @ BolsaError::UnauthorizedReviewer,
        constraint = sponsor_token_account.mint == mint.key() @ BolsaError::InvalidMint
    )]
    pub sponsor_token_account: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn handle(
    ctx: Context<InitializeGrant>,
    grant_id: u64,
    total_amount: u64,
    stream_percentage: u8,
    start_time: i64,
    end_time: i64,
    total_milestones: u8,
) -> Result<()> {
    require!(end_time > start_time, BolsaError::InvalidGrantDuration);
    require!(
        total_milestones > 0 && (total_milestones as usize) <= MAX_MILESTONES,
        BolsaError::InvalidMilestoneCount
    );
    require!(stream_percentage <= 100, BolsaError::MathOverflow);

    let stream_amount = (total_amount as u128)
        .checked_mul(stream_percentage as u128)
        .ok_or(BolsaError::MathOverflow)?
        .checked_div(100)
        .ok_or(BolsaError::MathOverflow)? as u64;

    let milestone_amount = total_amount
        .checked_sub(stream_amount)
        .ok_or(BolsaError::MathOverflow)?;

    let grant = &mut ctx.accounts.grant_account;
    grant.sponsor = ctx.accounts.sponsor.key();
    grant.beneficiary = ctx.accounts.beneficiary.key();
    grant.advisor = ctx.accounts.advisor.key();
    grant.mint = ctx.accounts.mint.key();
    grant.vault = ctx.accounts.escrow_vault.key();
    grant.grant_id = grant_id;
    grant.total_amount = total_amount;
    grant.stream_amount = stream_amount;
    grant.milestone_amount = milestone_amount;
    grant.claimed_amount = 0;
    grant.start_time = start_time;
    grant.end_time = end_time;
    grant.total_milestones = total_milestones;
    grant.milestones_approved = [false; MAX_MILESTONES];
    grant.status = GrantStatus::Active;
    grant.bump = ctx.bumps.grant_account;
    grant.vault_bump = ctx.bumps.escrow_vault;

    // CPI: Transfer funds from Sponsor ATA into Escrow Vault PDA
    let cpi_accounts = Transfer {
        from: ctx.accounts.sponsor_token_account.to_account_info(),
        to: ctx.accounts.escrow_vault.to_account_info(),
        authority: ctx.accounts.sponsor.to_account_info(),
    };
    let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
    token::transfer(cpi_ctx, total_amount)?;

    msg!(
        "Grant #{} Initialized. Total: {} (Stream: {}, Milestones: {})",
        grant_id,
        total_amount,
        stream_amount,
        milestone_amount
    );

    Ok(())
}