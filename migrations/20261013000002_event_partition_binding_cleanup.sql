-- DEFAULT repair moves events with DELETE + INSERT before ATTACH PARTITION.
-- Row-delete cleanup would destroy contracts for logically surviving events.
-- Repository deletes clean exact owner IDs in one statement; bounded retention
-- cleanup handles rows removed by partition DROP or boundary deletion.
DROP TRIGGER delete_event_schema_binding ON event;
DROP FUNCTION delete_owned_event_schema_binding();
