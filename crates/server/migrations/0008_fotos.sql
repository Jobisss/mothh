-- Foto de perfil: um PNG pequeno por pessoa. A versão sobe a cada troca, para
-- os apps saberem que a foto guardada ficou velha.
CREATE TABLE user_photos (
    user_id UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    png BYTEA NOT NULL,
    version BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
