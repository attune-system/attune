-- Repeated HTTP allocation of one rotation returns the same artifact version.
CREATE TABLE manager_sensor_log (
    workload BIGINT NOT NULL,
    generation BIGINT NOT NULL,
    request UUID NOT NULL,
    channel TEXT NOT NULL CHECK (channel IN ('stdout','stderr')),
    version BIGINT NOT NULL REFERENCES artifact_version(id) ON DELETE CASCADE,
    PRIMARY KEY (workload, generation, request, channel),
    FOREIGN KEY (workload, generation) REFERENCES manager_sensor_generation(workload,generation) ON DELETE CASCADE
);
