//! Retained canonical accounting for prepared fields. No render, source lookup
//! or authorization traversal is repeated to calculate a materialization budget.
use super::*;

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
