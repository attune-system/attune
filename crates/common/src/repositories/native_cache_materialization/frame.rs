//! Internal bridge for an owning native transition/wait stage. B owns the cache
//! declaration, session, canonical evaluation and destinations. A frame may bind
//! only its genuine issued VALUE/CONTROL sources; it never returns caller JSON.
use super::*;
use crate::repositories::inherited_input::RenderSession;

#[async_trait::async_trait]
pub(in crate::repositories) trait NativeCacheRenderFrame:
    Send + Sync
{
    fn execution(&self) -> Id;

    /// Optional reuse of a genuine source frame in the same exact source scope.
    /// None means this frame has no reusable Sources, not failed authorization.
    async fn prepared_sources(
        &self,
        _conn: &mut PgConnection,
        _parent: &Execution,
        _workflow: &WorkflowExecution,
        _key: &str,
    ) -> Result<Option<Arc<Sources>>> {
        Ok(None)
    }

    /// Source phase only. Do not evaluate, replace the session, scan Keys or
    /// reopen human authority. Bind actual staged values and original controls
    /// with the official inherited-input methods. B performs the one evaluation.
    async fn bind(
        &self,
        conn: &mut PgConnection,
        session: &mut RenderSession,
        binding: &DeclaredInputBinding,
        base: &Sources,
        key: &str,
    ) -> Result<()>;

    fn attach_literal(&self, rendered: &mut RenderedReceiptContext, base: &Sources) -> Result<()>;

    async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()>;
    fn ensure_fresh(&self) -> Result<()>;

    /// Target phase, after the owner compares its original state, releases its
    /// actual wait if applicable, and publishes/finalizes stages in order.
    /// Compare the owner's sealed publication buffers and normalized release
    /// facts only. No graph/source authorization or expression evaluation here.
    async fn require_published(&self, conn: &mut PgConnection) -> Result<()>;
}

#[async_trait::async_trait]
impl NativeCacheRenderFrame for PreparedNativeTransitions {
    fn execution(&self) -> Id {
        PreparedNativeTransitions::execution(self)
    }

    async fn prepared_sources(
        &self,
        conn: &mut PgConnection,
        parent: &Execution,
        workflow: &WorkflowExecution,
        key: &str,
    ) -> Result<Option<Arc<Sources>>> {
        Ok(Some(
            self.prepared_frame_sources(conn, parent, workflow, key)
                .await?,
        ))
    }

    async fn bind(
        &self,
        conn: &mut PgConnection,
        session: &mut RenderSession,
        binding: &DeclaredInputBinding,
        _base: &Sources,
        key: &str,
    ) -> Result<()> {
        self.bind_prepared_frame(conn, session, binding, key).await
    }

    fn attach_literal(&self, rendered: &mut RenderedReceiptContext, _base: &Sources) -> Result<()> {
        self.attach_frame_literal(rendered)
    }

    async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        PreparedNativeTransitions::ensure_fresh_on(self, conn).await
    }

    fn ensure_fresh(&self) -> Result<()> {
        PreparedNativeTransitions::ensure_fresh(self)
    }

    async fn require_published(&self, conn: &mut PgConnection) -> Result<()> {
        self.require_frame_publication(conn).await
    }
}
