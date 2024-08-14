-- Create bdk_descriptor_last_revealed table
CREATE TABLE bdk_descriptor_last_revealed (
    descriptor_id TEXT PRIMARY KEY NOT NULL,
    last_revealed BIGINT NOT NULL
);