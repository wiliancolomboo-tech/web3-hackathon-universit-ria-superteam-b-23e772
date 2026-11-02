use anchor_lang::prelude::*;

pub mod errors;
pub mod instructions;
pub mod state;

use instructions::*;

declare_id!("BoLSaFi111111111111111111111111111111111111");

#[program]
pub mod bolsafi {
    use super::*;

    pub fn initialize_grant(
        ctx: Context<InitializeGrant>,
        grant_id: u64,
        total_amount: u64,
        stream_percentage: u8,
        start_time: i64,
        end_time: i64,
        total_milestones: u8,
    ) -> Result<()> {
        instructions::initialize_grant::handle(
            ctx,
            grant_id,
            total_amount,
            stream_percentage,
            start_time,
            end_time,
            total_milestones,
        )
    }

    pub fn approve_milestone(ctx: Context<ApproveMilestone>, milestone_index: u8) -> Result<()> {
        instructions::approve_milestone::handle(ctx, milestone_index)
    }

    pub fn claim_funds(ctx: Context<ClaimFunds>) -> Result<()> {
        instructions::claim_funds::handle(ctx)
    }
}