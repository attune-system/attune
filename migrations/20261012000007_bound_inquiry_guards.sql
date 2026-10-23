CREATE FUNCTION guard_bound_inquiry_response() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM inquiry_schema_binding WHERE entity_id = OLD.id) THEN
        IF NEW.response_schema IS DISTINCT FROM OLD.response_schema OR NEW.response_options IS DISTINCT FROM OLD.response_options THEN
            RAISE EXCEPTION 'admitted inquiry schemas and options are immutable';
        END IF;
        IF OLD.status = 'responded' AND NEW.response IS DISTINCT FROM OLD.response THEN
            RAISE EXCEPTION 'accepted inquiry responses are immutable';
        END IF;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER guard_bound_inquiry_response BEFORE UPDATE ON inquiry
    FOR EACH ROW EXECUTE FUNCTION guard_bound_inquiry_response();
