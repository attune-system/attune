"""Deployment-local durable repository. Never reads or edits Attune's database."""
import sqlite3
from urllib.parse import quote
from contextlib import contextmanager

from protocol import canonical, decode, digest, require, MAX_REQUESTS


class Journal:
    @staticmethod
    def observe(path):
        """Read-only live-lane diagnostics of this deployment's own journal."""
        connection = sqlite3.connect("file:" + quote(str(path), safe="/") + "?mode=ro", uri=True)
        try:
            connection.execute("PRAGMA query_only=ON")
            row = connection.execute("SELECT binding,fenced FROM generation WHERE singleton=1").fetchone()
            require(row is not None, "JournalNotProvisioned")
            requests = connection.execute("SELECT operation,state FROM request ORDER BY operation LIMIT 257").fetchall()
            require(len(requests) <= MAX_REQUESTS, "RequestBound")
            return {"binding": decode(row[0]), "fenced": bool(row[1]),
                    "requests": dict(requests)}
        finally:
            connection.close()

    def __init__(self, path):
        self.db = sqlite3.connect(path, isolation_level=None)
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=FULL")
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS generation (
                singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                binding BLOB NOT NULL, fenced INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS request (
                operation TEXT PRIMARY KEY, member BLOB NOT NULL,
                mode TEXT NOT NULL, payload_digest TEXT NOT NULL,
                state TEXT NOT NULL CHECK(state IN ('accepted','joined','settled'))
            );
            CREATE TABLE IF NOT EXISTS audit (
                id TEXT PRIMARY KEY, plan BLOB NOT NULL, fence BLOB,
                inventory BLOB
            );
            CREATE TABLE IF NOT EXISTS deletion (
                audit TEXT NOT NULL, item_digest TEXT NOT NULL, item BLOB NOT NULL,
                absent INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(audit,item_digest)
            );
            CREATE TABLE IF NOT EXISTS enrollment (
                singleton INTEGER PRIMARY KEY CHECK(singleton=1), original BLOB NOT NULL
            );
        """)

    @contextmanager
    def transaction(self):
        self.db.execute("BEGIN IMMEDIATE")
        try:
            yield
            self.db.execute("COMMIT")
        except BaseException:
            self.db.execute("ROLLBACK")
            raise

    def bind(self, binding, initialize=False):
        row = self.db.execute("SELECT binding FROM generation WHERE singleton=1").fetchone()
        if row is None:
            require(initialize, "JournalNotProvisioned")
            self.db.execute("INSERT INTO generation(singleton,binding) VALUES(1,?)",
                            (canonical(binding),))
        else:
            require(decode(row[0]) == binding, "NativeGenerationChanged")

    def binding(self):
        row = self.db.execute("SELECT binding FROM generation WHERE singleton=1").fetchone()
        require(row is not None, "JournalNotProvisioned")
        return decode(row[0])

    def enroll(self, original):
        """Prospective original operation nonce, never a historical request."""
        with self.transaction():
            require(self.db.execute("SELECT fenced FROM generation").fetchone() == (0,), "GenerationFenced")
            require(self.db.execute("SELECT count(*) FROM request").fetchone()[0] == 0,
                    "GenerationAlreadyWriting")
            row = self.db.execute("SELECT original FROM enrollment WHERE singleton=1").fetchone()
            if row is not None:
                saved = decode(row[0])
                require({key: value for key, value in saved.items() if key != "operation"} == original,
                        "NativeEnrollmentRebound")
                return saved
            from uuid import uuid4
            saved = dict(original, operation=str(uuid4()))
            self.db.execute("INSERT INTO enrollment VALUES(1,?)", (canonical(saved),))
            return saved

    def require_enrollment(self, entry, mode):
        row = self.db.execute("SELECT original FROM enrollment WHERE singleton=1").fetchone()
        if row is not None:
            saved = decode(row[0])
            require(saved["operation"] == entry["operation"] and saved["mode"] == mode
                    and all(saved["writer"][key] == entry[key] for key in
                            ("native_instance", "worker_instance", "execution_attempt")),
                    "OriginalEnrollmentMismatch")

    def admit(self, entry, mode, payload_digest):
        with self.transaction():
            row = self.db.execute("SELECT member,mode,payload_digest,state FROM request WHERE operation=?",
                                  (entry["operation"],)).fetchone()
            if row is not None:
                require((decode(row[0]), row[1], row[2]) == (entry, mode, payload_digest),
                        "RequestIdentityRebound")
                # Lost replies never restart an accepted request, even if it
                # died before exec. The audit settles it under the native fence.
                return False
            require(self.db.execute("SELECT fenced FROM generation").fetchone() == (0,),
                    "GenerationFenced")
            require(self.db.execute("SELECT count(*) FROM request").fetchone()[0] < MAX_REQUESTS,
                    "RequestBound")
            for (raw,) in self.db.execute("SELECT member FROM request"):
                old = decode(raw)
                for key in ("object_key", "staging_path"):
                    require(entry[key] is None or old[key] != entry[key], "DestinationReused")
            self.db.execute("INSERT INTO request VALUES(?,?,?,?,'accepted')",
                            (entry["operation"], canonical(entry), mode, payload_digest))
            return True

    def joined(self, operation):
        self.db.execute("UPDATE request SET state='joined' WHERE operation=? AND state='accepted'",
                        (operation,))

    def fence(self, plan):
        # This commit precedes cgroup kill and survives a lost HTTP response or
        # daemon death. No request can ever reopen this generation.
        with self.transaction():
            recorded = [decode(row[0]) for row in self.db.execute("SELECT member FROM request")]
            require(sorted(recorded, key=digest) == sorted(plan["members"], key=digest),
                    "IncompleteGenerationPlan")
            row = self.db.execute("SELECT plan FROM audit WHERE id=?", (plan["id"],)).fetchone()
            if row is None:
                require(self.db.execute("SELECT count(*) FROM audit").fetchone()[0] == 0,
                        "GenerationAlreadyAudited")
                self.db.execute("INSERT INTO audit(id,plan) VALUES(?,?)",
                                (plan["id"], canonical(plan)))
            else:
                require(decode(row[0]) == plan, "AuditIdentityRebound")
            self.db.execute("UPDATE generation SET fenced=1")

    def audit(self, plan):
        row = self.db.execute("SELECT plan,fence,inventory FROM audit WHERE id=?",
                              (plan["id"],)).fetchone()
        require(row is not None and decode(row[0]) == plan, "UnknownAudit")
        return tuple(decode(value) if value is not None else None for value in row[1:])

    def settled(self, plan, fence, inventory):
        with self.transaction():
            previous, _ = self.audit(plan)
            require(previous is None or previous == fence, "FenceChanged")
            self.db.execute("UPDATE request SET state='settled'")
            self.db.execute("UPDATE audit SET fence=?,inventory=? WHERE id=? AND fence IS NULL",
                            (canonical(fence), canonical(inventory), plan["id"]))

    def delete_intent(self, plan, item):
        with self.transaction():
            _, inventory = self.audit(plan)
            require(inventory is not None and item in inventory, "UncataloguedIdentity")
            self.db.execute("INSERT OR IGNORE INTO deletion(audit,item_digest,item) VALUES(?,?,?)",
                            (plan["id"], digest(item), canonical(item)))

    def absent(self, plan, item):
        self.db.execute("UPDATE deletion SET absent=1 WHERE audit=? AND item_digest=?",
                        (plan["id"], digest(item)))

    def deletion_started(self, plan, item):
        return self.db.execute("SELECT 1 FROM deletion WHERE audit=? AND item_digest=?",
                               (plan["id"], digest(item))).fetchone() is not None
