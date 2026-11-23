CREATE INDEX manager_pack_job_release_pin ON manager_pack_job (release)
    WHERE release IS NOT NULL AND finish_digest IS NULL;
