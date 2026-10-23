//! Retained canonical accounting for prepared fields. No render, source lookup
//! or authorization traversal is repeated to calculate a materialization budget.
use super::*;

/// A union of the canonical reader's immutable per-object charges. Unlike the
/// ordinary activation reservation, shared ancestors are counted exactly once.
#[derive(Clone, Default)]
pub(crate) struct PreparedArchiveGraphBudget {
    charges: std::collections::BTreeMap<ReceiptGraphCharge, usize>,
    nodes: BTreeSet<(bool, Id)>,
    key_hash: Option<String>,
}
impl PreparedArchiveGraphBudget {
    fn include_accounting(&mut self, facts: &ReceiptGraphAccounting, key: &str) -> Result<()> {
        let key_hash = crate::crypto::hash_encryption_key(key);
        if facts.key_hash != key_hash
            || self.key_hash.as_ref().is_some_and(|held| held != &key_hash)
            || facts
                .charges
                .values()
                .try_fold(0usize, |sum, charge| sum.checked_add(*charge))
                .ok_or_else(denied)?
                != facts.bytes
        {
            return Err(denied());
        }
        self.key_hash = Some(key_hash);
        self.nodes.extend(facts.nodes.iter().copied());
        for (object, charge) in &facts.charges {
            if self
                .charges
                .insert(*object, *charge)
                .is_some_and(|held| held != *charge)
            {
                return Err(denied());
            }
        }
        self.require_capacity(0, 0)
    }
    pub(crate) fn include_snapshot(&mut self, snapshot: Id, bytes: usize) -> Result<()> {
        if self
            .charges
            .insert(ReceiptGraphCharge::Snapshot(snapshot), bytes)
            .is_some_and(|held| held != bytes)
        {
            return Err(denied());
        }
        self.nodes.insert((true, snapshot));
        self.require_capacity(0, 0)
    }
    pub(crate) fn require_capacity(
        &self,
        additional_bytes: usize,
        additional_nodes: usize,
    ) -> Result<()> {
        let bytes = self
            .charges
            .values()
            .try_fold(additional_bytes, |sum, charge| sum.checked_add(*charge))
            .ok_or_else(denied)?;
        let retained_nodes = self.nodes.len()
            + self
                .charges
                .keys()
                .filter(|charge| matches!(charge, ReceiptGraphCharge::Accepted(_)))
                .count();
        if bytes > 1024 * 1024
            || retained_nodes
                .checked_add(additional_nodes)
                .is_none_or(|nodes| nodes > 256)
        {
            return Err(Error::PermissionDenied(
                "Prepared archive graph budget exceeded".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct PreparedSourceGraphBudget {
    bytes: usize,
    nodes: BTreeSet<(bool, Id)>,
    key_hash: Option<String>,
}

impl PreparedSourceGraphBudget {
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }
    pub(super) fn node_count(&self) -> usize {
        self.nodes.len()
    }
    fn include_facts(
        &mut self,
        bytes: usize,
        nodes: &BTreeSet<(bool, Id)>,
        key_hash: &str,
    ) -> Result<()> {
        if self
            .key_hash
            .as_deref()
            .is_some_and(|expected| expected != key_hash)
        {
            return Err(denied());
        }
        self.key_hash.get_or_insert_with(|| key_hash.to_owned());
        self.bytes = self.bytes.checked_add(bytes).ok_or_else(denied)?;
        self.nodes.extend(nodes.iter().copied());
        // Same conservative byte fold and actual node identities as
        // prepared_source_graph_bounds. Reserve one owning publication node.
        if self.bytes > 1024 * 1024 || self.nodes.len() >= 256 {
            return Err(Error::PermissionDenied(
                "Prepared source graph budget exceeded".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn include(&mut self, other: &Self) -> Result<()> {
        if let Some(key_hash) = &other.key_hash {
            self.include_facts(other.bytes, &other.nodes, key_hash)?;
        } else if other.bytes != 0 || !other.nodes.is_empty() {
            return Err(denied());
        }
        Ok(())
    }
    fn include_accounting(&mut self, accounting: &ReceiptGraphAccounting, key: &str) -> Result<()> {
        if accounting.key_hash != crate::crypto::hash_encryption_key(key) {
            return Err(denied());
        }
        self.include_facts(accounting.bytes, &accounting.nodes, &accounting.key_hash)
    }
}

impl InheritedInputRepository {
    pub(crate) async fn prepared_archive_graph_budget(
        conn: &mut PgConnection,
        contexts: &[&ReceiptContext],
        key: &str,
    ) -> Result<PreparedArchiveGraphBudget> {
        let mut budget = PreparedArchiveGraphBudget::default();
        let mut admissions = RetainedReceiptAdmissions::default();
        for context in contexts {
            let facts = Self::held_graph_accounting(conn, context, key, &mut admissions).await?;
            budget.include_accounting(&facts, key)?;
        }
        Ok(budget)
    }
    pub(super) async fn prepare_archive_destination(
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        site: &InputSite,
        budget: &mut PreparedArchiveGraphBudget,
        key: &str,
    ) -> Result<PreparedReceiptDestination> {
        let (destination, facts) =
            Self::prepare_destination_with_graph_facts(conn, rendered, site, key).await?;
        for facts in facts {
            budget.include_accounting(facts.as_ref().ok_or_else(denied)?, key)?;
        }
        Ok(destination)
    }
    pub(crate) async fn prepare_destination_with_graph_budget(
        conn: &mut PgConnection,
        rendered: &RenderedReceiptContext,
        destination: &InputSite,
        key: &str,
    ) -> Result<(PreparedReceiptDestination, PreparedSourceGraphBudget)> {
        let (destination, facts) =
            Self::prepare_destination_with_graph_facts(conn, rendered, destination, key).await?;
        let mut budget = PreparedSourceGraphBudget::default();
        for accounting in facts {
            budget.include_accounting(accounting.as_ref().ok_or_else(denied)?, key)?;
        }
        Ok((destination, budget))
    }

    pub(crate) async fn include_prepared_control_graph_budget(
        conn: &mut PgConnection,
        budget: &mut PreparedSourceGraphBudget,
        source: &PreparedInputSource,
        key: &str,
    ) -> Result<()> {
        let mut admissions = RetainedReceiptAdmissions::default();
        for receipt in std::iter::once(&source.receipt).chain(&source.controls) {
            let accounting =
                Self::held_graph_accounting(conn, receipt, key, &mut admissions).await?;
            budget.include_accounting(&accounting, key)?;
        }
        Ok(())
    }

    pub(crate) async fn prepared_input_graph_budget(
        conn: &mut PgConnection,
        fields: &[&PreparedSourceGraphBudget],
        contexts: &[&ReceiptContext],
        key: &str,
    ) -> Result<PreparedSourceGraphBudget> {
        let mut budget = PreparedSourceGraphBudget::default();
        let mut admissions = RetainedReceiptAdmissions::default();
        for context in contexts {
            let accounting =
                Self::held_graph_accounting(conn, context, key, &mut admissions).await?;
            budget.include_accounting(&accounting, key)?;
        }
        for field in fields {
            budget.include(field)?;
        }
        Ok(budget)
    }
}
