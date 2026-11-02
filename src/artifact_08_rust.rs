use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::BolsaError;

#[derive(Accounts)]
pub partial struct ApproveMilestone<'info> {
    pub reviewer: Signer<'info>,

    #[account(
        mut,
        seeds = [GRANT_SEED, grant_account.sponsor.as_ref(), &grant_account.grant_id.to_le_bytes()],
        bump = grant_account.bump,
        constraint = grant_account.status == GrantStatus::Active @ BolsaError::GrantInactive
    )]
    pub grant_account: Account<'info, GrantAccount>,
}

pub fn handle(ctx: Context<ApproveMilestone>, milestone_index: u8) -> Result<()> {
    let grant = &mut ctx.accounts.grant_account;

    // Either academic advisor or sponsor has authorization to approve deliverable
    require!(
        ctx.accounts.reviewer.key() == grant.advisor || ctx.accounts.reviewer.key() == grant.sponsor,
        BolsaError::UnauthorizedReviewer
    );

    require!(
        milestone_index < grant.total_milestones,
        BolsaError::MilestoneInvalidOrAlreadyApproved
    );

    let idx = milestone_index as usize;
    require!(
        !grant.milestones_approved[idx],
        BolsaError::MilestoneInvalidOrAlreadyApproved
    );

    grant.milestones_approved[idx] = true;

    msg!(
        "Milestone #{} on Grant #{} successfully verified by {}",
        milestone_index,
        grant.grant_id,
        ctx.accounts.reviewer.key()
    );

    Ok(())
}