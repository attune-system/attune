-- One retained effect per existing sensor assignment. Never discard unknown
-- create/capture settlement when its lease or manager incarnation changes.
ALTER TABLE sensor_workload_assignment ADD COLUMN native_effect JSONB;
