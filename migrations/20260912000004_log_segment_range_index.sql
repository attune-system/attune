CREATE INDEX idx_log_segment_stream_byte_start
    ON log_segment (stream, byte_start);

CREATE INDEX idx_log_segment_stream_byte_end
    ON log_segment (stream, byte_end);
