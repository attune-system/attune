//! Carry source-validated approval deadlines through route target/query waits.
use attune_common::{
    repositories::runtime_authority::{ApprovalEffectFence, ValidatedRuntimeAuthority},
    Result,
};
use sqlx::{PgConnection, Postgres, Transaction};
use std::ops::{Deref, DerefMut};

/// An ordering/publication boundary, not an authorization permit. Callers must
/// validate source and targets normally. No source rows are relocked at commit.
pub struct EffectTransaction<'a> {
    transaction: Transaction<'a, Postgres>,
    fence: Option<ApprovalEffectFence>,
    authority: Option<ValidatedRuntimeAuthority>,
}

impl<'a> EffectTransaction<'a> {
    pub fn new(transaction: Transaction<'a, Postgres>, fence: Option<ApprovalEffectFence>) -> Self {
        Self {
            transaction,
            fence,
            authority: None,
        }
    }

    pub fn from_authority(
        transaction: Transaction<'a, Postgres>,
        authority: Option<ValidatedRuntimeAuthority>,
    ) -> Self {
        let fence = authority
            .as_ref()
            .map(|authority| authority.effect_fence().clone());
        Self {
            transaction,
            fence,
            authority,
        }
    }

    /// Reuse the original source validation after target locks. Never revalidate
    /// source rows merely to obtain authority for a second target in this effect.
    pub fn authorized_parts(&mut self) -> (&mut PgConnection, Option<&ValidatedRuntimeAuthority>) {
        (&mut self.transaction, self.authority.as_ref())
    }

    /// Call before nontransactional effects, after their own final wait.
    pub async fn ensure_fresh(&mut self) -> Result<()> {
        if let Some(fence) = &self.fence {
            fence.ensure_fresh_on(&mut self.transaction).await?;
        }
        Ok(())
    }

    pub async fn commit(mut self) -> Result<()> {
        self.ensure_fresh().await?;
        self.transaction.commit().await?;
        Ok(())
    }

    pub async fn rollback(self) -> Result<()> {
        self.transaction.rollback().await?;
        Ok(())
    }
}

impl Deref for EffectTransaction<'_> {
    type Target = PgConnection;
    fn deref(&self) -> &Self::Target {
        &self.transaction
    }
}

impl DerefMut for EffectTransaction<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.transaction
    }
}
