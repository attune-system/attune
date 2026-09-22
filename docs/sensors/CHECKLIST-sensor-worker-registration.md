# Sensor Worker Registration - Completion Checklist

**Feature:** Runtime capability reporting for sensor workers  
**Date:** 2026-01-31  
**Status:** Implementation Complete - Requires DB Migration

---

## Implementation Status

✅ **COMPLETE - Code Implementation**
- [x] Database migration created (`20260131000001_add_worker_role.sql`)
- [x] `WorkerRole` enum added to models
- [x] `Worker` model updated with `worker_role` field
- [x] `SensorConfig` struct added to config system
- [x] `SensorWorkerRegistration` module implemented
- [x] Service integration in `SensorService`
- [x] Runtime detection with 3-tier priority system
- [x] Heartbeat mechanism implemented
- [x] Graceful shutdown/deregistration
- [x] Unit tests included
- [x] Comprehensive documentation written

⚠️ **PENDING - Database & Testing**
- [ ] Database migration applied
- [ ] SQLx metadata regenerated
- [ ] Integration tests run
- [ ] Manual testing with live sensor service

---

## Required Steps to Complete

### Step 1: Start Database

```bash
# Ensure PostgreSQL is running
sudo systemctl start postgresql
# OR
docker-compose up -d postgres
```

### Step 2: Apply Migration

```bash
cd attune

# Set database URL
export DATABASE_URL="postgresql://postgres:postgres@localhost:5432/attune"

# Run migrations
sqlx migrate run

# Verify migration applied
psql $DATABASE_URL -c "\d worker"
# Should see: worker_role worker_role_enum NOT NULL
```

### Step 3: Regenerate SQLx Metadata

```bash
# With database running
cargo sqlx prepare --workspace

# Verify no compilation errors
cargo check --workspace
```

### Step 4: Manual Testing

```bash
# Terminal 1: Start sensor service
cargo run --bin attune-sensor

# Expected logs:
# - "Registering sensor worker..."
# - "Sensor worker registered with ID: X"
# - "Sensor worker heartbeat sent" (every 30s)

# Terminal 2: Query database
psql $DATABASE_URL -c "
  SELECT id, name, worker_role, status, 
         capabilities->'runtimes' as runtimes,
         last_heartbeat
  FROM worker 
  WHERE worker_role = 'sensor';
"

# Expected output:
# - One row with worker_role = 'sensor'
# - status = 'active'
# - runtimes array (e.g., ["python", "shell", "node", "native"])
# - Recent last_heartbeat timestamp

# Terminal 1: Stop sensor service (Ctrl+C)
# Expected log: "Deregistering sensor worker..."

# Terminal 2: Verify status changed
psql $DATABASE_URL -c "
  SELECT status FROM worker WHERE worker_role = 'sensor';
"
# Expected: status = 'inactive'
```

### Step 5: Test Runtime Detection

```bash
# Test auto-detection
cargo run --bin attune-sensor
# Check logs for "Auto-detected runtimes: ..."

# Test environment variable override
export ATTUNE_SENSOR_RUNTIMES="shell,native"
cargo run --bin attune-sensor
# Verify capabilities only include shell and native

# Test config file
cat > config.test-sensor.yaml <<EOF
sensor:
  worker_name: "test-sensor-01"
  capabilities:
    runtimes: ["python"]
  max_concurrent_sensors: 5
EOF

ATTUNE_CONFIG=config.test-sensor.yaml cargo run --bin attune-sensor
# Verify worker_name and runtimes from config
```

### Step 6: Test Heartbeat

```bash
# Start sensor service
cargo run --bin attune-sensor &
SENSOR_PID=$!

# Wait 2 minutes
sleep 120

# Check heartbeat updates
psql $DATABASE_URL -c "
  SELECT name, last_heartbeat, 
         NOW() - last_heartbeat as age
  FROM worker 
  WHERE worker_role = 'sensor';
"
# Expected: age should be < 30 seconds

# Cleanup
kill $SENSOR_PID
```

### Step 7: Integration Tests

```bash
# Run sensor service tests
cargo test --package attune-sensor

# Run integration tests after make db-test-setup
cargo test --package attune-sensor -- --test-threads=4

# Verify all tests pass
```

---

## Verification Checklist

### Database Schema
- [ ] `worker_role_enum` type exists with values: action, sensor
- [ ] `worker` table has `worker_role` column (NOT NULL)
- [ ] Indexes created: `idx_worker_role`, `idx_worker_role_status`
- [ ] Existing workers have `worker_role = 'action'`

### Configuration
- [ ] Can parse `sensor` config section from YAML
- [ ] `ATTUNE_SENSOR_RUNTIMES` env var works
- [ ] `ATTUNE__SENSOR__*` env var overrides work
- [ ] Auto-detection falls back correctly

### Registration
- [ ] Sensor service registers on startup
- [ ] Creates worker record with `worker_role = 'sensor'`
- [ ] Sets `status = 'active'`
- [ ] Populates `capabilities` with detected runtimes
- [ ] Records hostname in `host` field

### Heartbeat
- [ ] Heartbeat loop starts after registration
- [ ] `last_heartbeat` updates every 30s (default)
- [ ] Heartbeat interval configurable via config
- [ ] Errors logged but don't crash service

### Deregistration
- [ ] Service shutdown sets `status = 'inactive'`
- [ ] Worker record remains in database (not deleted)
- [ ] Deregistration logged

### Runtime Detection
- [ ] Auto-detects Python if `python3` or `python` available
- [ ] Auto-detects Node.js if `node` available
- [ ] Always includes "shell" and "native"
- [ ] Env var `ATTUNE_SENSOR_RUNTIMES` overrides all
- [ ] Config file `sensor.capabilities.runtimes` overrides auto-detection
- [ ] Detection priority: env var > config > auto-detect

---

## Known Issues / Limitations

### Current
- ✅ None - implementation is feature-complete

### Future Work
- 🔮 Distributed sensor scheduling not yet implemented (foundation is ready)
- 🔮 No automatic cleanup of stale workers (manual SQL required)
- 🔮 No API endpoints for querying sensor workers yet
- 🔮 Hybrid workers (action + sensor) not tested

---

## Rollback Plan

If issues arise:

```bash
# Rollback migration
sqlx migrate revert

# Remove worker_role column and enum
psql $DATABASE_URL -c "
  ALTER TABLE worker DROP COLUMN worker_role;
  DROP TYPE worker_role_enum;
"

# Revert code changes
git revert <commit-hash>
```

---

## Documentation Review

- [x] `docs/sensors/sensor-worker-registration.md` - Full documentation
- [x] `docs/QUICKREF-sensor-worker-registration.md` - Quick reference
- [x] `work-summary/sensor-worker-registration.md` - Implementation summary
- [x] This checklist created

---

## Sign-off

- [ ] Database migration applied and verified
- [ ] SQLx metadata regenerated
- [ ] All compilation warnings resolved
- [ ] Manual testing completed
- [ ] Integration tests pass
- [ ] Documentation reviewed
- [ ] AGENTS.md updated (if needed)
- [ ] Ready for production use

---

## Post-Deployment Monitoring

Once deployed, monitor:

```sql
-- Active sensor workers
SELECT COUNT(*) FROM worker 
WHERE worker_role = 'sensor' AND status = 'active';

-- Workers with stale heartbeat (> 2 minutes)
SELECT name, last_heartbeat, NOW() - last_heartbeat AS lag
FROM worker
WHERE worker_role = 'sensor'
  AND status = 'active'
  AND last_heartbeat < NOW() - INTERVAL '2 minutes';

-- Runtime distribution
SELECT 
  jsonb_array_elements_text(capabilities->'runtimes') AS runtime,
  COUNT(*) AS worker_count
FROM worker
WHERE worker_role = 'sensor' AND status = 'active'
GROUP BY runtime;
```

---

**Next Session:** Apply migration, test with live database, verify all checks pass
