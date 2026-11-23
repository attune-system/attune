ALTER TABLE manager_sensor_generation
    ADD COLUMN definition_updated TIMESTAMPTZ,
    ADD COLUMN release BIGINT,
    ADD COLUMN started_pid INTEGER CHECK (started_pid > 0),
    ADD COLUMN finished_as TEXT CHECK (finished_as IN ('stopped','failed'));
