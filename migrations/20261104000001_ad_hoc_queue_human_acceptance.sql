-- Human enqueue has historical attribution and an admitted target Action, but
-- no originating execution admission. Execution-origin items retain that FK.
ALTER TABLE work_queue_item_ad_hoc_dispatch ALTER COLUMN source_admission DROP NOT NULL;
ALTER TABLE runtime_accepted_batch_item ALTER COLUMN source_admission DROP NOT NULL;
