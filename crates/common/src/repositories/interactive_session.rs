//! Interactive credentials only. Machine credentials retain their own provenance.
use sha2::{Digest, Sha256};
use sqlx::PgPool;

pub struct InteractiveSessionRepository;

pub fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

impl InteractiveSessionRepository {
    /// Fence issuance against concurrent password changes and identity freezing.
    pub async fn create(
        pool: &PgPool,
        identity: &crate::models::Identity,
        token: &str,
        ttl: i64,
    ) -> crate::Result<bool> {
        let mut tx = pool.begin().await?;
        let eligible: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM identity WHERE id = $1 AND NOT frozen AND password_hash IS NOT DISTINCT FROM $2 FOR UPDATE)")
            .bind(identity.id).bind(&identity.password_hash).fetch_one(&mut *tx).await?;
        if eligible {
            let hash = token_hash(token);
            let id: i64 = sqlx::query_scalar("INSERT INTO interactive_session(identity_id,current_hash,expires) VALUES($1,$2,clock_timestamp()+$3*interval '1 second') RETURNING id")
                .bind(identity.id).bind(&hash).bind(ttl).fetch_one(&mut *tx).await?;
            sqlx::query("INSERT INTO interactive_refresh_use(token_hash,session_id) VALUES($1,$2)")
                .bind(hash)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(eligible)
    }

    /// A replay commits family revocation before returning false. Row locking makes
    /// concurrent uses deterministic: one rotation, then family revocation.
    pub async fn rotate(
        pool: &PgPool,
        identity_id: i64,
        old: &str,
        new: &str,
    ) -> crate::Result<bool> {
        let mut tx = pool.begin().await?;
        let active: Option<i64> =
            sqlx::query_scalar("SELECT id FROM identity WHERE id=$1 AND NOT frozen FOR UPDATE")
                .bind(identity_id)
                .fetch_optional(&mut *tx)
                .await?;
        if active.is_none() {
            tx.commit().await?;
            return Ok(false);
        }
        let row: Option<(i64,String,bool,bool)> = sqlx::query_as("SELECT s.id,s.current_hash,s.revoked,s.expires>clock_timestamp() FROM interactive_session s JOIN interactive_refresh_use u ON u.session_id=s.id WHERE u.token_hash=$1 AND s.identity_id=$2 FOR UPDATE OF s")
            .bind(token_hash(old)).bind(identity_id).fetch_optional(&mut *tx).await?;
        let mut accepted = false;
        if let Some((id, current, revoked, unexpired)) = row {
            if !revoked && unexpired && current == token_hash(old) {
                let hash = token_hash(new);
                sqlx::query("UPDATE interactive_session SET current_hash=$1 WHERE id=$2")
                    .bind(&hash)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(
                    "INSERT INTO interactive_refresh_use(token_hash,session_id) VALUES($1,$2)",
                )
                .bind(hash)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                accepted = true;
            } else {
                sqlx::query("UPDATE interactive_session SET revoked=TRUE WHERE id=$1")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(accepted)
    }

    pub async fn revoke(pool: &PgPool, token: &str) -> crate::Result<()> {
        sqlx::query("UPDATE interactive_session SET revoked=TRUE WHERE id IN (SELECT session_id FROM interactive_refresh_use WHERE token_hash=$1)")
            .bind(token_hash(token)).execute(pool).await?;
        Ok(())
    }

    pub async fn create_exchange(
        pool: &PgPool,
        identity_id: i64,
        code: &str,
        browser: &str,
        payload: &str,
        redirect: &str,
        refresh: &str,
    ) -> crate::Result<()> {
        let mut tx = pool.begin().await?;
        let active: Option<i64> =
            sqlx::query_scalar("SELECT id FROM identity WHERE id=$1 AND NOT frozen FOR UPDATE")
                .bind(identity_id)
                .fetch_optional(&mut *tx)
                .await?;
        if active.is_none() {
            return Err(crate::Error::not_found(
                "identity",
                "id",
                identity_id.to_string(),
            ));
        }
        let result = sqlx::query("INSERT INTO interactive_login_exchange(code_hash,browser_hash,identity_id,payload,redirect_to) SELECT $1,$2,$3,$4,$5 WHERE EXISTS(SELECT 1 FROM interactive_session WHERE identity_id=$3 AND current_hash=$6 AND NOT revoked AND expires>clock_timestamp())")
            .bind(token_hash(code)).bind(token_hash(browser)).bind(identity_id).bind(payload).bind(redirect).bind(token_hash(refresh)).execute(&mut *tx).await?;
        if result.rows_affected() != 1 {
            return Err(crate::Error::not_found(
                "session",
                "identity",
                identity_id.to_string(),
            ));
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn consume_exchange(
        pool: &PgPool,
        code: &str,
        browser: &str,
    ) -> crate::Result<Option<(String, String)>> {
        Ok(sqlx::query_as("DELETE FROM interactive_login_exchange e USING identity i WHERE e.code_hash=$1 AND e.browser_hash=$2 AND e.expires>clock_timestamp() AND i.id=e.identity_id AND NOT i.frozen RETURNING e.payload,e.redirect_to")
            .bind(token_hash(code)).bind(token_hash(browser)).fetch_optional(pool).await?)
    }
}
