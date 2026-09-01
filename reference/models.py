import enum
from datetime import datetime
from typing import ClassVar, Literal, NotRequired, Protocol, TypedDict, TypeVar

from alembic_utils.pg_function import PGFunction
from alembic_utils.pg_grant_table import PGGrantTable
from alembic_utils.pg_trigger import PGTrigger
from sqlalchemy import (
    ARRAY,
    BigInteger,
    Boolean,
    CheckConstraint,
    DateTime,
    Enum,
    ForeignKey,
    Index,
    Integer,
    MetaData,
    Text,
    func,
)
from sqlalchemy.dialects.postgresql import JSONB
from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column, relationship
from sqlalchemy.sql.schema import Constraint

from .schema_utils import JSONDict, JSONList, SchemaDict

DB_SCHEMA = "attune"
SCHEMA_METADATA = MetaData(schema=DB_SCHEMA)
SERVICE_ROLE = "svc_attune"


class Base(DeclarativeBase):
    metadata: ClassVar[MetaData] = SCHEMA_METADATA


SCHEMA_FUNCTIONS: list[PGFunction] = [  # type: ignore
    PGFunction(
        schema=DB_SCHEMA,
        signature=(CROSSFILL_PACK := "cross_fill_pack_ref()"),
        definition=f"""RETURNS TRIGGER AS $$
    DECLARE
        ref_pack TEXT;
        pack_id BIGINT;
    BEGIN
        -- Auto-fill pack_ref if not provided but ref contains pack prefix
        IF (NEW.pack_ref IS NULL AND NEW.ref IS NOT NULL AND NEW.ref ~ '\\.') THEN
            NEW.pack_ref := split_part(NEW.ref, '.', 1);
        ELSIF NEW.ref IS NOT NULL AND NEW.ref !~ '\\.' AND NEW.pack_ref IS NOT NULL THEN
            NEW.ref := NEW.pack_ref || '.' || NEW.ref;
        END IF;

        -- Lookup and set pack ID if pack_ref is provided
        IF (NEW.pack_ref IS NOT NULL AND NEW.pack IS NULL) THEN
            SELECT id INTO pack_id FROM {DB_SCHEMA}.pack WHERE ref = NEW.pack_ref;
            IF FOUND THEN
                NEW.pack := pack_id;
            END IF;
        END IF;

        RETURN NEW;
    END;
    $$ language 'plpgsql'""",
    ),
    PGFunction(
        schema=DB_SCHEMA,
        signature=(POPULATE_FOREIGN_REFS := "populate_foreign_refs()"),
        definition="""RETURNS TRIGGER AS $$
        -- Handle foreign key reference population dynamically
    DECLARE
        col_name TEXT;
        ref_col_name TEXT;
        fk_table TEXT;
        fk_value BIGINT;
        ref_value TEXT;
        old_fk_value BIGINT;
        sql_query TEXT;
    BEGIN
        -- Loop through all columns ending with '_ref'
        FOR col_name IN
            SELECT column_name
            FROM information_schema.columns
            WHERE table_schema = TG_TABLE_SCHEMA
            AND table_name = TG_TABLE_NAME
            AND column_name LIKE '%_ref'
        LOOP
            -- Get the corresponding foreign key column name (remove '_ref' suffix)
            ref_col_name := substring(col_name from 1 for length(col_name) - 4);

            -- Check if the corresponding FK column exists
            IF EXISTS (
                SELECT 1 FROM information_schema.columns
                WHERE table_schema = TG_TABLE_SCHEMA
                AND table_name = TG_TABLE_NAME
                AND column_name = ref_col_name
            ) THEN
                -- Get the FK value from NEW record
                EXECUTE format('SELECT ($1).%I', ref_col_name) INTO fk_value USING NEW;

                -- Get old FK value for UPDATE operations
                IF TG_OP = 'UPDATE' THEN
                    EXECUTE format('SELECT ($1).%I', ref_col_name) INTO old_fk_value USING OLD;
                END IF;

                -- Only proceed if FK value exists and (it's INSERT or FK value changed)
                IF fk_value IS NOT NULL AND (TG_OP = 'INSERT' OR fk_value != COALESCE(old_fk_value, -1)) THEN
                    -- Get the table name from information_schema
                    SELECT tc.table_name INTO fk_table
                    FROM information_schema.table_constraints tc
                    JOIN information_schema.key_column_usage kcu
                        ON tc.constraint_name = kcu.constraint_name
                        AND tc.table_schema = kcu.table_schema
                    JOIN information_schema.constraint_column_usage ccu
                        ON ccu.constraint_name = tc.constraint_name
                        AND ccu.table_schema = tc.table_schema
                    WHERE tc.constraint_type = 'FOREIGN KEY'
                        AND tc.table_schema = TG_TABLE_SCHEMA
                        AND tc.table_name = TG_TABLE_NAME
                        AND kcu.column_name = ref_col_name;

                    -- If we found the FK table, look up the ref value
                    IF fk_table IS NOT NULL THEN
                        sql_query := format('SELECT ref FROM %I.%I WHERE id = $1', TG_TABLE_SCHEMA, fk_table);
                        EXECUTE sql_query INTO ref_value USING fk_value;

                        -- Set the ref column value
                        IF ref_value IS NOT NULL THEN
                            EXECUTE format('SELECT ($1) #= hstore(%I, $2)', col_name)
                            INTO NEW
                            USING NEW, ref_value;
                        END IF;
                    END IF;
                END IF;
            END IF;
        END LOOP;

        RETURN NEW;
    END;
    $$ language 'plpgsql'""",
    ),
]

SCHEMA_TRIGGERS: list[PGTrigger] = [  # type: ignore
    PGTrigger(
        schema=DB_SCHEMA,
        signature="cross_fill_sensor_refs",
        on_entity=f"{DB_SCHEMA}.sensor",
        definition=f"""BEFORE INSERT OR UPDATE ON {DB_SCHEMA}.sensor
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{CROSSFILL_PACK}""",
    ),
    PGTrigger(
        schema=DB_SCHEMA,
        signature="cross_fill_action_refs",
        on_entity=f"{DB_SCHEMA}.action",
        definition=f"""BEFORE INSERT OR UPDATE ON {DB_SCHEMA}.action
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{CROSSFILL_PACK}""",
    ),
    PGTrigger(
        schema=DB_SCHEMA,
        signature="cross_fill_rule_refs",
        on_entity=f"{DB_SCHEMA}.rule",
        definition=f"""BEFORE INSERT OR UPDATE ON {DB_SCHEMA}.rule
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{CROSSFILL_PACK}""",
    ),
    PGTrigger(
        schema=DB_SCHEMA,
        signature="cross_fill_permission_set_refs",
        on_entity=f"{DB_SCHEMA}.permission_set",
        definition=f"""BEFORE INSERT OR UPDATE ON {DB_SCHEMA}.permission_set
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{CROSSFILL_PACK}""",
    ),
    PGTrigger(
        schema=DB_SCHEMA,
        signature="cross_fill_trigger_refs",
        on_entity=f"{DB_SCHEMA}.trigger",
        definition=f"""BEFORE INSERT OR UPDATE ON {DB_SCHEMA}.trigger
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{CROSSFILL_PACK}""",
    ),
    PGTrigger(
        schema=DB_SCHEMA,
        signature="cross_fill_policy_refs",
        on_entity=f"{DB_SCHEMA}.policy",
        definition=f"""BEFORE INSERT OR UPDATE ON {DB_SCHEMA}.policy
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{CROSSFILL_PACK}""",
    ),
    PGTrigger(
        schema=DB_SCHEMA,
        signature="cross_fill_runtime_refs",
        on_entity=f"{DB_SCHEMA}.runtime",
        definition=f"""BEFORE INSERT OR UPDATE ON {DB_SCHEMA}.runtime
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{CROSSFILL_PACK}""",
    ),
    PGTrigger(
        schema=DB_SCHEMA,
        signature="cross_fill_key_refs",
        on_entity=f"{DB_SCHEMA}.key",
        definition=f"""BEFORE INSERT OR UPDATE ON {DB_SCHEMA}.key
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{CROSSFILL_PACK}""",
    ),
    PGTrigger(
        schema=DB_SCHEMA,
        signature="populate_sensor_refs",
        on_entity=f"{DB_SCHEMA}.sensor",
        definition=f"""BEFORE INSERT OR UPDATE ON {DB_SCHEMA}.sensor
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{POPULATE_FOREIGN_REFS}""",
    ),
    PGTrigger(
        schema=DB_SCHEMA,
        signature="populate_action_refs",
        on_entity=f"{DB_SCHEMA}.action",
        definition=f"""BEFORE INSERT OR UPDATE ON {DB_SCHEMA}.action
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{POPULATE_FOREIGN_REFS}""",
    ),
    PGTrigger(
        schema=DB_SCHEMA,
        signature="populate_rule_refs",
        on_entity=f"{DB_SCHEMA}.rule",
        definition=f"""BEFORE INSERT OR UPDATE ON {DB_SCHEMA}.rule
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{POPULATE_FOREIGN_REFS}""",
    ),
    PGTrigger(
        schema=DB_SCHEMA,
        signature="populate_policy_refs",
        on_entity=f"{DB_SCHEMA}.policy",
        definition=f"""BEFORE INSERT OR UPDATE ON {DB_SCHEMA}.policy
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{POPULATE_FOREIGN_REFS}""",
    ),
    PGTrigger(
        schema=DB_SCHEMA,
        signature="populate_key_refs",
        on_entity=f"{DB_SCHEMA}.key",
        definition=f"""BEFORE INSERT OR UPDATE ON {DB_SCHEMA}.key
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{POPULATE_FOREIGN_REFS}""",
    ),
]

SCHEMA_TABLE_GRANTS: list[PGGrantTable] = []


class Pack(Base):
    __tablename__: str = "pack"
    __table_args__: tuple[Constraint, ...] = (
        CheckConstraint("ref ~ '^[a-z][a-z0-9_-]+$'", name="pack_ref_lowercase"),
        CheckConstraint(
            "version ~ '^\\d+\\.\\d+\\.\\d+(-[0-9A-Za-z-]+(\\.[0-9A-Za-z-]+)*)?(\\+[0-9A-Za-z-]+(\\.[0-9A-Za-z-]+)*)?$'",
            name="pack_version_semver",
        ),
    )

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    ref: Mapped[str] = mapped_column(Text, nullable=False, unique=True)
    label: Mapped[str] = mapped_column(Text, nullable=False)
    description: Mapped[str | None] = mapped_column(Text)
    version: Mapped[str] = mapped_column(Text, nullable=False)
    conf_schema: Mapped[SchemaDict] = mapped_column(JSONB, nullable=False, default={})
    config: Mapped[JSONDict] = mapped_column(JSONB, nullable=False, default={})
    meta: Mapped[JSONDict] = mapped_column(JSONB, nullable=False, default={})
    tags: Mapped[list[str]] = mapped_column(ARRAY(Text), nullable=False, default=[])
    runtime_deps: Mapped[list[str]] = mapped_column(
        ARRAY(Text), nullable=False, default=[]
    )
    is_standard: Mapped[bool] = mapped_column(Boolean, nullable=False, default=False)
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )

    # Relationships
    runtimes: Mapped[list["Runtime"]] = relationship(
        back_populates="dependent_packs", cascade="all, delete-orphan"
    )
    triggers: Mapped[list["Trigger"]] = relationship(
        back_populates="pack_obj", cascade="all, delete-orphan"
    )
    sensors: Mapped[list["Sensor"]] = relationship(
        back_populates="pack_obj", cascade="all, delete-orphan"
    )
    actions: Mapped[list["Action"]] = relationship(
        back_populates="pack_obj", cascade="all, delete-orphan"
    )
    rules: Mapped[list["Rule"]] = relationship(
        back_populates="pack_obj", cascade="all, delete-orphan"
    )
    permission_sets: Mapped[list["PermissionSet"]] = relationship(
        back_populates="pack_obj", cascade="all, delete-orphan"
    )
    policies: Mapped[list["Policy"]] = relationship(
        back_populates="pack_obj", cascade="all, delete-orphan"
    )


class Runtime(Base):
    __tablename__: str = "runtime"
    __table_args__: tuple[Constraint, ...] = (
        CheckConstraint("ref = lower(ref)", name="runtime_ref_lowercase"),
    )

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    ref: Mapped[str] = mapped_column(Text, nullable=False, unique=True)
    pack: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("pack.id", ondelete="CASCADE"), nullable=True
    )
    pack_ref: Mapped[str | None] = mapped_column(Text, nullable=True)
    description: Mapped[str | None] = mapped_column(Text)
    name: Mapped[str] = mapped_column(Text, nullable=False)
    distributions: Mapped[JSONDict] = mapped_column(JSONB, nullable=False)
    installation: Mapped[JSONDict | None] = mapped_column(JSONB)
    execution_config: Mapped[JSONDict | None] = mapped_column(JSONB)
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )

    # Relationships
    dependent_packs: Mapped[list[Pack]] = relationship(back_populates="runtimes")
    workers: Mapped[list["Worker"]] = relationship(back_populates="runtime_obj")
    sensors: Mapped[list["Sensor"]] = relationship(back_populates="runtime_obj")
    actions: Mapped[list["Action"]] = relationship(back_populates="runtime_obj")


class WorkerType(enum.Enum):
    local = "local"
    remote = "remote"
    container = "container"


class WorkerStatus(enum.Enum):
    active = "active"
    inactive = "inactive"
    busy = "busy"
    error = "error"


class Worker(Base):
    __tablename__: str = "worker"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    name: Mapped[str] = mapped_column(Text, nullable=False)
    worker_type: Mapped[WorkerType] = mapped_column(
        Enum(WorkerType, name="worker_type_enum", schema=DB_SCHEMA), nullable=False
    )
    runtime: Mapped[int | None] = mapped_column(BigInteger, ForeignKey("runtime.id"))
    host: Mapped[str | None] = mapped_column(Text)
    port: Mapped[int | None] = mapped_column(Integer)
    status: Mapped[WorkerStatus | None] = mapped_column(
        Enum(WorkerStatus, name="worker_status_enum", schema=DB_SCHEMA),
        default="inactive",
    )
    capabilities: Mapped[JSONDict | None] = mapped_column(JSONB)
    meta: Mapped[JSONDict | None] = mapped_column(JSONB)
    last_heartbeat: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )

    # Relationships
    runtime_obj: Mapped[Runtime | None] = relationship(back_populates="workers")


class Trigger(Base):
    __tablename__: str = "trigger"
    __table_args__: tuple[Constraint, ...] = (
        CheckConstraint("ref = lower(ref)", name="trigger_ref_lowercase"),
        CheckConstraint("ref ~ '^[^.]+\\.[^.]+$'", name="trigger_ref_format"),
    )

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    ref: Mapped[str] = mapped_column(Text, nullable=False, unique=True)
    pack: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("pack.id", ondelete="CASCADE"), nullable=True
    )
    pack_ref: Mapped[str | None] = mapped_column(Text, nullable=True)
    label: Mapped[str] = mapped_column(Text, nullable=False)
    description: Mapped[str | None] = mapped_column(Text)
    enabled: Mapped[bool] = mapped_column(Boolean, nullable=False, default=True)
    param_schema: Mapped[SchemaDict | None] = mapped_column(JSONB)
    out_schema: Mapped[SchemaDict | None] = mapped_column(JSONB)
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )

    # Relationships
    pack_obj: Mapped[Pack | None] = relationship(back_populates="triggers")
    sensors: Mapped[list["Sensor"]] = relationship(back_populates="trigger_obj")
    rules: Mapped[list["Rule"]] = relationship(back_populates="trigger_obj")


class Sensor(Base):
    __tablename__: str = "sensor"
    __table_args__: tuple[Constraint, ...] = (
        CheckConstraint("ref = lower(ref)", name="sensor_ref_lowercase"),
        CheckConstraint("ref ~ '^[^.]+\\.[^.]+$'", name="sensor_ref_format"),
    )

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    ref: Mapped[str] = mapped_column(Text, nullable=False, unique=True)
    pack: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("pack.id", ondelete="CASCADE"), nullable=True
    )
    pack_ref: Mapped[str | None] = mapped_column(Text, nullable=True)
    label: Mapped[str] = mapped_column(Text, nullable=False)
    description: Mapped[str] = mapped_column(Text, nullable=False)
    entrypoint: Mapped[str] = mapped_column(Text, nullable=False)
    runtime: Mapped[int] = mapped_column(
        BigInteger, ForeignKey("runtime.id"), nullable=False
    )
    runtime_ref: Mapped[str] = mapped_column(Text, nullable=False)
    trigger: Mapped[int] = mapped_column(
        BigInteger, ForeignKey("trigger.id"), nullable=False
    )
    trigger_ref: Mapped[str] = mapped_column(Text, nullable=False)
    enabled: Mapped[bool] = mapped_column(Boolean, nullable=False)
    param_schema: Mapped[SchemaDict | None] = mapped_column(JSONB)
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )

    # Relationships
    pack_obj: Mapped[Pack | None] = relationship(back_populates="sensors")
    runtime_obj: Mapped[Runtime] = relationship(back_populates="sensors")
    trigger_obj: Mapped[Trigger] = relationship(back_populates="sensors")


class Action(Base):
    __tablename__: str = "action"
    __table_args__: tuple[Constraint, ...] = (
        CheckConstraint("ref = lower(ref)", name="action_ref_lowercase"),
        CheckConstraint("ref ~ '^[^.]+\\.[^.]+$'", name="action_ref_format"),
    )

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    ref: Mapped[str] = mapped_column(Text, nullable=False, unique=True)
    pack: Mapped[int] = mapped_column(
        BigInteger, ForeignKey("pack.id", ondelete="CASCADE"), nullable=False
    )
    pack_ref: Mapped[str] = mapped_column(Text, nullable=False)
    label: Mapped[str] = mapped_column(Text, nullable=False)
    description: Mapped[str] = mapped_column(Text, nullable=False)
    entrypoint: Mapped[str] = mapped_column(Text, nullable=False)
    runtime: Mapped[int | None] = mapped_column(BigInteger, ForeignKey("runtime.id"))
    param_schema: Mapped[SchemaDict | None] = mapped_column(JSONB)
    out_schema: Mapped[SchemaDict | None] = mapped_column(JSONB)
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )

    # Relationships
    pack_obj: Mapped[Pack] = relationship(back_populates="actions")
    runtime_obj: Mapped[Runtime | None] = relationship(back_populates="actions")
    rules: Mapped[list["Rule"]] = relationship(back_populates="action_obj")


class Rule(Base):
    __tablename__: str = "rule"
    __table_args__: tuple[Constraint, ...] = (
        CheckConstraint("ref = lower(ref)", name="rule_ref_lowercase"),
        CheckConstraint("ref ~ '^[^.]+\\.[^.]+$'", name="rule_ref_format"),
    )

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    ref: Mapped[str] = mapped_column(Text, nullable=False, unique=True)
    pack: Mapped[int] = mapped_column(
        BigInteger, ForeignKey("pack.id", ondelete="CASCADE"), nullable=False
    )
    pack_ref: Mapped[str] = mapped_column(Text, nullable=False)
    label: Mapped[str] = mapped_column(Text, nullable=False)
    description: Mapped[str] = mapped_column(Text, nullable=False)
    action: Mapped[int] = mapped_column(
        BigInteger, ForeignKey("action.id"), nullable=False
    )
    action_ref: Mapped[str] = mapped_column(Text, nullable=False)
    trigger: Mapped[int] = mapped_column(
        BigInteger, ForeignKey("trigger.id"), nullable=False
    )
    trigger_ref: Mapped[str] = mapped_column(Text, nullable=False)
    conditions: Mapped[JSONList] = mapped_column(JSONB, nullable=False, default=[])
    enabled: Mapped[bool] = mapped_column(Boolean, nullable=False)
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )

    # Relationships
    pack_obj: Mapped[Pack] = relationship(back_populates="rules")
    action_obj: Mapped[Action] = relationship(back_populates="rules")
    trigger_obj: Mapped[Trigger] = relationship(back_populates="rules")


class PermissionGrant(TypedDict):
    type: Literal["system", "pack", "user"]
    scope: NotRequired[str]  # name of the pack for pack-scoped permissions
    components: NotRequired[list[str]]


class PermissionSet(Base):
    __tablename__: str = "permission_set"
    __table_args__: tuple[Constraint, ...] = (
        CheckConstraint("ref = lower(ref)", name="role_ref_lowercase"),
        CheckConstraint("ref ~ '^[^.]+\\.[^.]+$'", name="role_ref_format"),
    )

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    ref: Mapped[str] = mapped_column(Text, nullable=False, unique=True)
    pack: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("pack.id", ondelete="CASCADE"), nullable=True
    )
    pack_ref: Mapped[str | None] = mapped_column(Text, nullable=True)
    label: Mapped[str | None] = mapped_column(Text)
    description: Mapped[str | None] = mapped_column(Text)
    grants: Mapped[list[PermissionGrant]] = mapped_column(
        JSONB, nullable=False, default=[]
    )
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )

    # Relationships
    pack_obj: Mapped[Pack | None] = relationship(back_populates="permission_sets")
    assignments: Mapped[list["PermissionAssignment"]] = relationship(
        back_populates="permission_set_obj"
    )


class PolicyMethod(enum.Enum):
    cancel = "cancel"
    enqueue = "enqueue"


class Policy(Base):
    __tablename__: str = "policy"
    __table_args__: tuple[Constraint, ...] = (
        CheckConstraint("ref = lower(ref)", name="policy_ref_lowercase"),
        CheckConstraint("ref ~ '^[^.]+\\.[^.]+$'", name="policy_ref_format"),
        CheckConstraint("threshold > 0", name="policy_threshold_positive"),
    )

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    ref: Mapped[str] = mapped_column(Text, nullable=False, unique=True)
    pack: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("pack.id", ondelete="CASCADE"), nullable=True
    )
    pack_ref: Mapped[str | None] = mapped_column(Text, nullable=True)
    action: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("action.id", ondelete="CASCADE"), nullable=True
    )
    action_ref: Mapped[str | None] = mapped_column(Text, nullable=True)
    parameters: Mapped[list[str]] = mapped_column(
        ARRAY(Text), nullable=False, default=[]
    )
    method: Mapped[PolicyMethod] = mapped_column(
        Enum(PolicyMethod, name="policy_method_enum", schema=DB_SCHEMA), nullable=False
    )
    threshold: Mapped[int] = mapped_column(Integer, nullable=False)
    name: Mapped[str] = mapped_column(Text, nullable=False)
    description: Mapped[str | None] = mapped_column(Text)
    tags: Mapped[list[str]] = mapped_column(ARRAY(Text), nullable=False, default=[])
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )

    # Relationships
    pack_obj: Mapped[Pack | None] = relationship(back_populates="policies")


class Event(Base):
    __tablename__: str = "event"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    trigger: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("trigger.id", ondelete="SET NULL")
    )
    trigger_ref: Mapped[str] = mapped_column(Text, nullable=False, index=True)
    config: Mapped[JSONDict | None] = mapped_column(JSONB)
    payload: Mapped[JSONDict | None] = mapped_column(JSONB, nullable=True)
    source: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("sensor.id"), nullable=True
    )
    source_ref: Mapped[str | None] = mapped_column(Text)
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )


SCHEMA_FUNCTIONS.extend(
    [
        PGFunction(
            schema=DB_SCHEMA,
            signature=(_sig := "validate_event_references()"),
            definition=f"""
            RETURNS TRIGGER AS $$
            BEGIN
                -- Validate trigger exists (required)
                IF NOT EXISTS (SELECT 1 FROM {DB_SCHEMA}."trigger" WHERE id = NEW."trigger") THEN
                    RAISE EXCEPTION 'Referenced trigger % does not exist', NEW."trigger";
                END IF;

                -- Validate source exists if provided
                IF NEW.source IS NOT NULL AND NOT EXISTS (SELECT 1 FROM {DB_SCHEMA}.sensor WHERE id = NEW.source) THEN
                    RAISE EXCEPTION 'Referenced source % does not exist', NEW.source;
                END IF;

                RETURN NEW;
            END;
            $$ language 'plpgsql'
        """,
        ),
        PGFunction(
            schema=DB_SCHEMA,
            signature=(_sig2 := "capture_event_config()"),
            definition=f"""
            RETURNS TRIGGER AS $$
            DECLARE
                trigger_config JSONB;
                source_config JSONB;
                final_config JSONB := '{{}}';
            BEGIN
                -- Capture trigger configuration
                SELECT jsonb_build_object(
                    'id', t.id,
                    'pack', t.pack,
                    'name', t.name,
                    'label', t.label,
                    'description', t.description,
                    'enabled', t.enabled,
                    'param_schema', t.param_schema,
                    'out_schema', t.out_schema
                ) INTO trigger_config
                FROM {DB_SCHEMA}.trigger t WHERE t.id = NEW.trigger;

                final_config := jsonb_set(final_config, '{{trigger}}', trigger_config);

                -- Capture source/sensor configuration if provided
                IF NEW.source IS NOT NULL THEN
                    SELECT jsonb_build_object(
                        'id', s.id,
                        'pack', s.pack,
                        'name', s.name,
                        'description', s.description,
                        'entrypoint', s.entrypoint,
                        'runtime', s.runtime,
                        'triggers', s.triggers,
                        'filepath', s.filepath,
                        'enabled', s.enabled,
                        'param_schema', s.param_schema
                    ) INTO source_config
                    FROM {DB_SCHEMA}.sensor s WHERE s.id = NEW.source;

                    final_config := jsonb_set(final_config, '{{source}}', source_config);
                END IF;

                NEW.config := final_config;
                RETURN NEW;
            END;
            $$ language 'plpgsql'""",
        ),
    ]
)

SCHEMA_TRIGGERS.extend(
    [
        PGTrigger(
            schema=DB_SCHEMA,
            signature="validate_event_refs",
            on_entity=f"{DB_SCHEMA}.event",
            definition=f"""BEFORE INSERT ON {DB_SCHEMA}.event
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{_sig}""",
        ),
        PGTrigger(
            schema=DB_SCHEMA,
            signature="capture_event_config",
            on_entity=f"{DB_SCHEMA}.event",
            definition=f"""BEFORE INSERT ON {DB_SCHEMA}.event
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{_sig2}""",
        ),
    ]
)


class EnforcementStatus(enum.Enum):
    created = "created"
    processed = "processed"
    disabled = "disabled"


class EnforcementCondition(enum.Enum):
    any = "any"
    all = "all"


class Enforcement(Base):
    __tablename__: str = "enforcement"
    __table_args__: tuple[Constraint, ...] = (
        CheckConstraint(
            "condition IN ('any', 'all')", name="enforcement_condition_check"
        ),
    )

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    rule: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("rule.id", ondelete="SET NULL")
    )
    rule_ref: Mapped[str] = mapped_column(Text, nullable=False, index=True)
    trigger_ref: Mapped[str] = mapped_column(Text, nullable=False, index=True)
    config: Mapped[JSONDict | None] = mapped_column(JSONB)
    event: Mapped[int | None] = mapped_column(BigInteger, ForeignKey("event.id"))
    status: Mapped[EnforcementStatus] = mapped_column(
        Enum(EnforcementStatus, name="enforcement_status_enum", schema=DB_SCHEMA),
        nullable=False,
        default="created",
    )
    payload: Mapped[JSONDict] = mapped_column(JSONB, nullable=False)
    condition: Mapped[EnforcementCondition] = mapped_column(
        Enum(EnforcementCondition, name="enforcement_condition_enum", schema=DB_SCHEMA),
        nullable=False,
        default=EnforcementCondition.all,
    )
    conditions: Mapped[JSONList] = mapped_column(JSONB, nullable=False, default=[])
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )

    # Relationships
    execution_obj: Mapped[list["Execution"]] = relationship(
        back_populates="enforcement_obj"
    )


# Validate that the rule exists on enforcement insert
SCHEMA_FUNCTIONS.extend(
    [
        PGFunction(
            schema=DB_SCHEMA,
            signature=(_sig3 := "capture_enforcement_config()"),
            definition=f"""RETURNS TRIGGER AS $$
    DECLARE
        rule_config JSONB;
        trigger_config JSONB;
        event_config JSONB;
        final_config JSONB := '{{}}';
    BEGIN
        -- Capture rule configuration
        SELECT jsonb_build_object(
            'id', r.id,
            'pack', r.pack,
            'name', r.name,
            'description', r.description,
            'action', r.action,
            'trigger', r.trigger,
            'trigger_ref', r.trigger_ref,
            'conditions', r.conditions,
            'enabled', r.enabled
        ) INTO rule_config
        FROM {DB_SCHEMA}.rule r WHERE r.id = NEW.instanceof;

        SELECT jsonb_build_object(
            'id', t.id,
            'ref', t.ref,
            'pack', t.pack,
            'pack_ref', t.pack_ref,
            'description', t.description,
            'label', t.label,
            'out_schema', t.out_schema
        ) into trigger_config
        FROM {DB_SCHEMA}.trigger t WHERE t.id = rule_config ->> 'trigger';

        SELECT jsonb_build_object(
            'trigger_ref', e.trigger_ref,
            'config', e.config,
            'source_ref', e.source_ref
        ) into event_config
        FROM {DB_SCHEMA}.event e WHERE e.id = NEW.event;

        -- Store only event ID for runtime reference
        final_config := jsonb_set(final_config, '{{event}}', to_jsonb(NEW.event));
        final_config := jsonb_set(final_config, '{{rule}}', rule_config);
        final_config := jsonb_set(final_config, '{{trigger}}', rule_config);

        NEW.config := final_config;
        RETURN NEW;
    END;
    $$ language 'plpgsql'""",
        ),
    ]
)

SCHEMA_TRIGGERS.extend(
    [
        PGTrigger(
            schema=DB_SCHEMA,
            signature="capture_enforcement_config",
            on_entity=f"{DB_SCHEMA}.enforcement",
            definition=f"""BEFORE INSERT ON {DB_SCHEMA}.enforcement
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{_sig3}""",
        ),
    ]
)


class ExecutionStatus(enum.Enum):
    requested = "requested"
    scheduling = "scheduling"
    scheduled = "scheduled"
    running = "running"
    completed = "completed"
    failed = "failed"
    canceling = "canceling"
    cancelled = "cancelled"
    timeout = "timeout"
    abandoned = "abandoned"


class Execution(Base):
    __tablename__: str = "execution"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    action: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("action.id"), index=True
    )
    action_ref: Mapped[str] = mapped_column(Text, nullable=False, index=True)
    config: Mapped[JSONDict | None] = mapped_column(JSONB)
    parent: Mapped[int | None] = mapped_column(BigInteger, ForeignKey("execution.id"))
    enforcement: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("enforcement.id")
    )
    executor: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("identity.id"), index=True
    )
    status: Mapped[ExecutionStatus] = mapped_column(
        Enum(ExecutionStatus, name="execution_status_enum", schema=DB_SCHEMA),
        nullable=False,
        default="requested",
    )
    result: Mapped[JSONDict | None] = mapped_column(JSONB)
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )

    # Relationships
    parent_workflow: Mapped["Execution | None"] = relationship(
        "Execution", remote_side=[id], back_populates="tasks"
    )
    tasks: Mapped[list["Execution"]] = relationship(
        "Execution", back_populates="parent_workflow"
    )
    enforcement_obj: Mapped[Enforcement | None] = relationship(
        back_populates="execution_obj"
    )
    executor_obj: Mapped["Identity | None"] = relationship(back_populates="executions")
    inquiries: Mapped[list["Inquiry"]] = relationship(
        back_populates="execution_obj", cascade="all, delete-orphan"
    )


SCHEMA_FUNCTIONS.extend(
    [
        PGFunction(
            schema=DB_SCHEMA,
            signature=(_sig4 := "capture_execution_config()"),
            definition=f"""RETURNS TRIGGER AS $$
    DECLARE
        action_config JSONB;
        user_config JSONB;
        final_config JSONB := '{{}}';
    BEGIN
        -- Capture action configuration
        SELECT jsonb_build_object(
            'id', a.id,
            'pack', a.pack,
            'name', a.name,
            'description', a.description,
            'entrypoint', a.entrypoint,
            'runtime', a.runtime,
            'param_schema', a.param_schema,
            'out_schema', a.out_schema
        ) INTO action_config
        FROM {DB_SCHEMA}.action a WHERE a.id = NEW.action;

        final_config := jsonb_set(final_config, '{{action}}', action_config);

        -- Capture user configuration if provided
        IF NEW.user IS NOT NULL THEN
            SELECT jsonb_build_object(
                'id', u.id,
                'username', u.username
            ) INTO user_config
            FROM {DB_SCHEMA}.users u WHERE u.id = NEW.user;

            final_config := jsonb_set(final_config, '{{user}}', user_config);
        END IF;

        -- Store only IDs for runtime references
        IF NEW.parent IS NOT NULL THEN
            final_config := jsonb_set(final_config, '{{parent}}', to_jsonb(NEW.parent));
        END IF;

        IF NEW.enforcement IS NOT NULL THEN
            final_config := jsonb_set(final_config, '{{enforcement}}', to_jsonb(NEW.enforcement));
        END IF;

        NEW.config := final_config;
        RETURN NEW;
    END;
    $$ language 'plpgsql'""",
        ),
    ]
)

SCHEMA_TRIGGERS.extend(
    [
        PGTrigger(
            schema=DB_SCHEMA,
            signature="capture_execution_config",
            on_entity=f"{DB_SCHEMA}.execution",
            definition=f"""BEFORE INSERT ON {DB_SCHEMA}.execution
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{_sig4}""",
        ),
    ]
)


class InquiryStatus(enum.Enum):
    pending = "pending"
    responded = "responded"
    timeout = "timeout"
    cancelled = "cancelled"


class Inquiry(Base):
    __tablename__: str = "inquiry"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    execution: Mapped[int] = mapped_column(
        BigInteger, ForeignKey("execution.id", ondelete="CASCADE"), nullable=False
    )
    prompt: Mapped[str] = mapped_column(Text, nullable=False)
    response_schema: Mapped[SchemaDict | None] = mapped_column(JSONB)
    assigned_to: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("identity.id", ondelete="SET NULL")
    )
    status: Mapped[InquiryStatus] = mapped_column(
        Enum(InquiryStatus, name="inquiry_status_enum", schema=DB_SCHEMA),
        nullable=False,
        default=InquiryStatus.pending,
    )
    response: Mapped[JSONDict | None] = mapped_column(JSONB)
    timeout_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    responded_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )

    # Relationships
    execution_obj: Mapped[Execution] = relationship(back_populates="inquiries")
    assigned_to_obj: Mapped["Identity | None"] = relationship(
        back_populates="inquiries"
    )


class OwnerType(enum.Enum):
    system = "system"
    identity = "identity"
    pack = "pack"
    action = "action"
    sensor = "sensor"


_PgOwnerTypeEnum = Enum(OwnerType, name="owner_type_enum", schema=DB_SCHEMA)


class Key(Base):
    __tablename__: str = "key"
    __table_args__: tuple[Constraint | Index, ...] = (
        CheckConstraint(
            "local_ref ~ '^[a-z0-9][a-z0-9_-]{0,62}$'",
            name="key_local_ref_format",
        ),
        CheckConstraint(
            "ref = canonical_key_ref(owner_type, "
            "owner, owner_pack_ref, owner_action_ref, owner_sensor_ref, local_ref)",
            name="key_ref_canonical",
        ),
        # Unique constraint on owner_type, owner, name
        Index("idx_key_unique", "owner_type", "owner", "name", unique=True),
    )

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    ref: Mapped[str] = mapped_column(Text, nullable=False, unique=True)
    local_ref: Mapped[str] = mapped_column(Text, nullable=False)
    owner_type: Mapped[OwnerType] = mapped_column(
        _PgOwnerTypeEnum,
        nullable=False,
        index=True,
    )
    owner: Mapped[str | None] = mapped_column(Text)
    owner_identity: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("identity.id")
    )
    owner_pack: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("pack.id"), nullable=True
    )
    owner_pack_ref: Mapped[str | None] = mapped_column(Text, nullable=True)
    owner_action: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("action.id"), nullable=True
    )
    owner_action_ref: Mapped[str | None] = mapped_column(Text, nullable=True)
    owner_sensor: Mapped[int | None] = mapped_column(
        BigInteger, ForeignKey("sensor.id"), nullable=True
    )
    owner_sensor_ref: Mapped[str | None] = mapped_column(Text, nullable=True)
    name: Mapped[str] = mapped_column(Text, nullable=False)
    encrypted: Mapped[bool] = mapped_column(Boolean, nullable=False)
    encryption_key_hash: Mapped[str | None] = mapped_column(Text)
    value: Mapped[str] = mapped_column(Text, nullable=False)
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )


SCHEMA_FUNCTIONS.append(
    PGFunction(
        schema=DB_SCHEMA,
        signature=(
            "canonical_key_ref(owner_type_enum, TEXT, TEXT, TEXT, TEXT, TEXT)"
        ),
        definition="""RETURNS TEXT AS $$
        BEGIN
            RETURN CASE $1
                WHEN 'system' THEN 'system.' || $6
                WHEN 'identity' THEN 'identity.' || $2 || '.' || $6
                WHEN 'pack' THEN 'pack.' || $3 || '.' || $6
                WHEN 'action' THEN 'action.' || $4 || '.' || $6
                WHEN 'sensor' THEN 'sensor.' || $5 || '.' || $6
            END;
        END;
        $$ LANGUAGE plpgsql IMMUTABLE""",
    )
)


# Add function to validate and set owner fields
SCHEMA_FUNCTIONS.append(
    PGFunction(
        schema=DB_SCHEMA,
        signature="validate_key_owner()",
        definition="""RETURNS TRIGGER AS $$
        DECLARE
            owner_count INTEGER := 0;
        BEGIN
            IF TG_OP = 'UPDATE' AND (
                NEW.owner_type IS DISTINCT FROM OLD.owner_type
                OR NEW.owner_identity IS DISTINCT FROM OLD.owner_identity
                OR NEW.owner_pack IS DISTINCT FROM OLD.owner_pack
                OR NEW.owner_action IS DISTINCT FROM OLD.owner_action
                OR NEW.owner_sensor IS DISTINCT FROM OLD.owner_sensor
                OR NEW.local_ref IS DISTINCT FROM OLD.local_ref
            ) THEN
                RAISE EXCEPTION 'Key owner and local_ref cannot be changed';
            END IF;

            IF NEW.owner_identity IS NOT NULL THEN owner_count := owner_count + 1; END IF;
            IF NEW.owner_pack IS NOT NULL THEN owner_count := owner_count + 1; END IF;
            IF NEW.owner_action IS NOT NULL THEN owner_count := owner_count + 1; END IF;
            IF NEW.owner_sensor IS NOT NULL THEN owner_count := owner_count + 1; END IF;

            IF NEW.owner_type = 'system' THEN
                IF owner_count > 0 THEN
                    RAISE EXCEPTION 'System owner cannot have specific owner fields set';
                END IF;
                NEW.owner := 'system';
                NEW.owner_pack_ref := NULL;
                NEW.owner_action_ref := NULL;
                NEW.owner_sensor_ref := NULL;
            ELSIF owner_count != 1 THEN
                RAISE EXCEPTION 'Exactly one owner field must be set for owner_type %', NEW.owner_type;
            ELSIF NEW.owner_type = 'identity' THEN
                IF NEW.owner_identity IS NULL THEN
                    RAISE EXCEPTION 'owner_identity must be set for owner_type identity';
                END IF;
                SELECT login INTO STRICT NEW.owner FROM identity WHERE id = NEW.owner_identity;
                NEW.owner_pack_ref := NULL;
                NEW.owner_action_ref := NULL;
                NEW.owner_sensor_ref := NULL;
            ELSIF NEW.owner_type = 'pack' THEN
                IF NEW.owner_pack IS NULL THEN
                    RAISE EXCEPTION 'owner_pack must be set for owner_type pack';
                END IF;
                SELECT ref INTO STRICT NEW.owner_pack_ref FROM pack WHERE id = NEW.owner_pack;
                NEW.owner := NEW.owner_pack_ref;
                NEW.owner_action_ref := NULL;
                NEW.owner_sensor_ref := NULL;
            ELSIF NEW.owner_type = 'action' THEN
                IF NEW.owner_action IS NULL THEN
                    RAISE EXCEPTION 'owner_action must be set for owner_type action';
                END IF;
                SELECT ref INTO STRICT NEW.owner_action_ref FROM action WHERE id = NEW.owner_action;
                NEW.owner := NEW.owner_action_ref;
                NEW.owner_pack_ref := NULL;
                NEW.owner_sensor_ref := NULL;
            ELSIF NEW.owner_type = 'sensor' THEN
                IF NEW.owner_sensor IS NULL THEN
                    RAISE EXCEPTION 'owner_sensor must be set for owner_type sensor';
                END IF;
                SELECT ref INTO STRICT NEW.owner_sensor_ref FROM sensor WHERE id = NEW.owner_sensor;
                NEW.owner := NEW.owner_sensor_ref;
                NEW.owner_pack_ref := NULL;
                NEW.owner_action_ref := NULL;
            END IF;

            NEW.ref := canonical_key_ref(
                NEW.owner_type,
                NEW.owner,
                NEW.owner_pack_ref,
                NEW.owner_action_ref,
                NEW.owner_sensor_ref,
                NEW.local_ref
            );
            RETURN NEW;
        END;
        $$ language 'plpgsql'""",
    )
)

# Add trigger to validate owner fields
SCHEMA_TRIGGERS.append(
    PGTrigger(
        schema=DB_SCHEMA,
        signature="validate_key_owner_trigger",
        on_entity=f"{DB_SCHEMA}.key",
        definition=f"""BEFORE INSERT OR UPDATE ON {DB_SCHEMA}.key
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.validate_key_owner()""",
    )
)


class Identity(Base):
    __tablename__: str = "identity"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    login: Mapped[str] = mapped_column(Text, unique=True, nullable=False)
    display_name: Mapped[str | None] = mapped_column(Text)
    attributes: Mapped[JSONDict] = mapped_column(JSONB, nullable=False, default={})
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )

    # Relationships
    executions: Mapped[list[Execution]] = relationship(back_populates="executor_obj")
    permissions: Mapped[list["PermissionAssignment"]] = relationship(
        back_populates="identity_obj"
    )
    inquiries: Mapped[list[Inquiry]] = relationship(back_populates="assigned_to_obj")


class PermissionAssignment(Base):
    __tablename__: str = "permission_assignment"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    identity: Mapped[int] = mapped_column(
        BigInteger, ForeignKey("identity.id"), nullable=False
    )
    permset: Mapped[int] = mapped_column(
        BigInteger, ForeignKey("permission_set.id"), nullable=False
    )
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )

    # Relationships
    identity_obj: Mapped[Identity] = relationship(back_populates="permissions")
    permission_set_obj: Mapped[PermissionSet] = relationship(
        back_populates="assignments"
    )


class NotificationState(enum.Enum):
    created = "created"
    queued = "queued"
    processing = "processing"
    error = "error"


class Notification(Base):
    __tablename__: str = "notification"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    channel: Mapped[str] = mapped_column(Text, nullable=False)
    entity_type: Mapped[str] = mapped_column(Text, nullable=False)
    entity: Mapped[str] = mapped_column(Text, nullable=False)
    activity: Mapped[str] = mapped_column(Text, nullable=False)
    state: Mapped[NotificationState] = mapped_column(
        Enum(NotificationState, name="notification_status_enum", schema=DB_SCHEMA),
        nullable=False,
        default=NotificationState.created,
    )
    content: Mapped[JSONDict | None] = mapped_column(JSONB)
    created: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now()
    )
    updated: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=func.now(), onupdate=func.now()
    )


# Add notification function and trigger
SCHEMA_FUNCTIONS.append(
    PGFunction(
        schema=DB_SCHEMA,
        signature="notify_on_insert()",
        definition="""RETURNS TRIGGER AS $$
        DECLARE
            payload TEXT;
        BEGIN
            -- Build JSON payload with id, entity, and activity
            payload := json_build_object(
                'id', NEW.id,
                'entity_type', NEW.entity_type,
                'entity', NEW.entity,
                'activity', NEW.activity
            )::text;

            -- Send notification to the specified channel
            PERFORM pg_notify(NEW.channel, payload);

            RETURN NEW;
        END;
        $$ language 'plpgsql'""",
    )
)

SCHEMA_TRIGGERS.append(
    PGTrigger(
        schema=DB_SCHEMA,
        signature="notify_on_notification_insert",
        on_entity=f"{DB_SCHEMA}.notification",
        definition=f"""AFTER INSERT ON {DB_SCHEMA}.notification
            FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.notify_on_insert()""",
    )
)


# Additional functions
SCHEMA_FUNCTIONS.extend(
    [
        PGFunction(
            schema=DB_SCHEMA,
            signature=(PACK_EXISTS := "validate_component_pack_exists()"),
            definition=f"""RETURNS TRIGGER AS $$
    DECLARE
        ref_pack TEXT;
    BEGIN
        -- Validate pack reference exists if pack_ref is provided
        IF NEW.pack_ref IS NOT NULL AND NOT EXISTS (SELECT 1 FROM {DB_SCHEMA}.pack WHERE ref = NEW.pack_ref) THEN
            RAISE EXCEPTION 'Referenced pack % does not exist', NEW.pack_ref;
        END IF;

        -- Validate that pack_ref column matches pack extracted FROM {DB_SCHEMA}.ref
        -- All component tables now have mandatory pack.name ref format
        IF NEW.ref IS NOT NULL AND NEW.ref ~ '\\.' THEN
            ref_pack := split_part(NEW.ref, '.', 1);
            IF NEW.pack_ref IS NOT NULL AND ref_pack != NEW.pack_ref THEN
                RAISE EXCEPTION 'Pack in ref (%) does not match pack_ref column (%)', ref_pack, NEW.pack_ref;
            END IF;
        END IF;

        RETURN NEW;
    END;
    $$ language 'plpgsql'""",
        ),
        PGFunction(
            schema=DB_SCHEMA,
            signature=(NOTIFY_COMPONENT_CHANGES := "notify_component_changes()"),
            definition=f"""RETURNS TRIGGER AS $$
    DECLARE
        rec RECORD;
    BEGIN
        -- Use OLD for DELETE operations, NEW for INSERT/UPDATE
        IF TG_OP = 'DELETE' THEN
            rec := OLD;
        ELSE
            rec := NEW;
        END IF;

        -- Create notification record instead of direct pg_notify
        INSERT INTO {DB_SCHEMA}.notification (channel, entity_type, entity, activity)
        VALUES (
            TG_TABLE_NAME,
            TG_TABLE_NAME,
            rec.ref,
            lower(TG_OP)
        );
        RETURN rec;
    END;
    $$ language 'plpgsql'""",
        ),
        PGFunction(
            schema=DB_SCHEMA,
            signature=(NOTIFY_INSTANCE_CREATED := "notify_instance_created") + "()",
            definition=f"""RETURNS TRIGGER AS $$
    DECLARE _fk_col TEXT := TG_ARGV[0];
    DECLARE _ref_col TEXT := TG_ARGV[1];
    DECLARE _NEW JSONB := to_jsonb(NEW);
    BEGIN
    -- Create notification record instead of direct pg_notify
    INSERT INTO {DB_SCHEMA}.notification (channel, entity_type, entity, activity, content)
    VALUES (
        TG_TABLE_NAME,
        TG_TABLE_NAME,
        NEW.id,
        'created',
        jsonb_build_object(
            _fk_col, _NEW ->> _fk_col,
            _ref_col, _NEW ->> _ref_col
        )
    );
    RETURN NEW;
    END;
    $$ language 'plpgsql'""",
        ),
        PGFunction(
            schema=DB_SCHEMA,
            signature=(NOTIFY_STATUS_CHANGED := "notify_status_changed()"),
            definition=f"""RETURNS TRIGGER AS $$
    BEGIN
    -- Create notification record instead of direct pg_notify
    INSERT INTO {DB_SCHEMA}.notification (channel, entity_type, entity, activity)
    VALUES (
        TG_TABLE_NAME,
        TG_TABLE_NAME,
        NEW.id,
        NEW.status
    );
    RETURN NEW;
    END;
    $$ language 'plpgsql'""",
        ),
    ]
)


# Setup instance table fields and grants
instance_table_fields = {
    "event": ("trigger", "trigger_ref"),
    "enforcement": ("rule", "rule_ref"),
    "execution": ("action", "action_ref"),
}

for table in list(SCHEMA_METADATA.tables.values()):
    SCHEMA_TABLE_GRANTS.extend(
        [
            PGGrantTable(
                schema=DB_SCHEMA,
                table=table.name,
                role=SERVICE_ROLE,
                with_grant_option=True,
                columns=[*table.columns.keys()],
                grant=grant,
            )
            for grant in ["INSERT", "REFERENCES", "SELECT", "UPDATE"]
        ]
    )
    SCHEMA_TABLE_GRANTS.extend(
        [
            PGGrantTable(
                schema=DB_SCHEMA,
                table=table.name,
                role=SERVICE_ROLE,
                with_grant_option=True,
                grant=grant,
            )
            for grant in ["DELETE", "TRUNCATE", "TRIGGER"]
        ]
    )

    if "status" in table.columns:
        SCHEMA_TRIGGERS.append(
            PGTrigger(
                schema=DB_SCHEMA,
                signature="notify_status_update",
                on_entity=table.key,
                definition=f"""AFTER UPDATE OF status ON {table.key}
                    FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{NOTIFY_STATUS_CHANGED}""",
            )
        )

    if table.key in instance_table_fields:
        SCHEMA_TRIGGERS.append(
            PGTrigger(
                schema=DB_SCHEMA,
                signature="notify_instance_created",
                on_entity=table.key,
                definition=f"""AFTER INSERT ON {table.key}
                    FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{NOTIFY_INSTANCE_CREATED}('{instance_table_fields[table.key][0]}', '{instance_table_fields[table.key][1]}')""",
            )
        )

    if "pack" in table.columns:
        SCHEMA_TRIGGERS.extend(
            [
                PGTrigger(
                    schema=DB_SCHEMA,
                    signature="constrain_pack_exists",
                    on_entity=table.key,
                    is_constraint=True,
                    definition=f"""AFTER INSERT OR UPDATE ON {table.key}
                    FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{PACK_EXISTS}""",
                ),
                PGTrigger(
                    schema=DB_SCHEMA,
                    signature="notify_on_update",
                    on_entity=table.key,
                    definition=f"""AFTER INSERT OR UPDATE OR DELETE ON {table.key}
                    FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{NOTIFY_COMPONENT_CHANGES}""",
                ),
            ]
        )

    if any(
        str(col)[: -len("_ref")] in table.columns
        for col in table.columns
        if str(col).endswith("_ref")
    ):
        SCHEMA_TRIGGERS.append(
            PGTrigger(
                schema=DB_SCHEMA,
                signature="populate_foreign_refs",
                on_entity=table.key,
                is_constraint=True,
                definition=f"""BEFORE INSERT OR UPDATE ON {table.key}
                FOR EACH ROW EXECUTE FUNCTION {DB_SCHEMA}.{POPULATE_FOREIGN_REFS}('{table.key}')""",
            )
        )


class ArtifactType(enum.Enum):
    file_binary = "file::binary"  # to be downloaded as a binary file from the UI
    file_datatable = "file::datatable"  # to be displayed as a datatable in the UI
    file_image = "file::image"  # to be displayed as an image in the UI
    file_text = "file::text"  # to be displayed as plain text in the UI
    other = "other"
    progress = "progress"
    url = "url"


class RetentionPolicyType(enum.Enum):
    versions = "versions"  # retain a specific number of versions
    days = "days"  # retain for a specific number of days
    hours = "hours"  # retain for a specific number of hours
    minutes = "minutes"  # retain for a specific number of minutes


class Artifact(Base):
    __tablename__: str = "artifact"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    ref: Mapped[str] = mapped_column(Text, nullable=False)
    scope: Mapped[OwnerType] = mapped_column(
        _PgOwnerTypeEnum,
        nullable=False,
        default=OwnerType.system,
    )
    owner: Mapped[str] = mapped_column(Text, nullable=False, default="")
    type: Mapped[ArtifactType] = mapped_column(
        Enum(ArtifactType, name="artifact_type_enum", schema=DB_SCHEMA), nullable=False
    )
    retention_policy: Mapped[RetentionPolicyType] = mapped_column(
        Enum(ArtifactType, name="artifact_retention_enum", schema=DB_SCHEMA),
        nullable=False,
        default=RetentionPolicyType.versions,
    )
    retention_limit: Mapped[int] = mapped_column(Integer, nullable=False, default=1)


_T_covar = TypeVar("_T_covar", covariant=True)


class _TBL(Protocol[_T_covar]):
    __tablename__: str
    id: Mapped[int]


class _CMP_TBL(_TBL[_T_covar], Protocol[_T_covar]):
    ref: Mapped[str]


DB_Table = TypeVar("DB_Table", bound=_TBL[Base])
DB_Cmp_Table = TypeVar("DB_Cmp_Table", bound=_CMP_TBL[Base])


table_object_map: dict[str, type[_TBL[Base]]] = {  # pyright: ignore[reportAssignmentType]
    obj.__tablename__.lower(): obj  # pyright: ignore[reportAny]
    for obj in Base.__subclasses__()
    if isinstance(getattr(obj, "__tablename__", None), str)
}
