use anchor_lang::prelude::*;

#[error_code]
pub enum BolsaError {
    #[msg("Calculation overflow detected in mathematical operation.")]
    MathOverflow,
    #[msg("Invalid grant duration: end time must be strictly after start time.")]
    InvalidGrantDuration,
    #[msg("Total milestones cannot be zero or exceed the absolute limit of 10.")]
    InvalidMilestoneCount,
    #[msg("Signer is not authorized to review or approve this milestone.")]
    UnauthorizedReviewer,
    #[msg("Specified milestone index is out of bounds or already approved.")]
    MilestoneInvalidOrAlreadyApproved,
    #[msg("No unlocked funds available for withdrawal at this timestamp.")]
    NoAvailableFunds,
    #[msg("Grant is currently frozen, completed, or cancelled.")]
    GrantInactive,
    #[msg("Grant expiration has not elapsed; cancellation threshold unmet.")]
    GrantNotExpired,
    #[msg("Invalid token mint supplied; must match designated grant currency.")]
    InvalidMint,
    #[msg("Grant funds are already fully disbursed.")]
    GrantFullyClaimed,
}