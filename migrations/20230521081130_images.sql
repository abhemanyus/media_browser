-- Add migration script here
CREATE TABLE IF NOT EXISTS images (
    image_id VARCHAR(500),
    name VARCHAR(500) NOT NULL,
    src VARCHAR(500) NOT NULL,
    size INTEGER NOT NULL,
    format VARCHAR(10) NOT NULL,
    CONSTRAINT images_pkey PRIMARY KEY (image_id),
    CONSTRAINT src_unique UNIQUE (name),
    CONSTRAINT format_check CHECK (LOWER(format) IN ('png', 'jpg', 'jpeg', 'webp')),
    CONSTRAINT size_check CHECK(size > 0)
);
