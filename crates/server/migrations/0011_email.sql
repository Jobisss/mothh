-- V6-04: e-mail na conta e código para recuperar a senha.
-- Contas antigas ficam sem e-mail (NULL) e continuam entrando normalmente;
-- podem pôr um depois, no perfil.
ALTER TABLE users ADD COLUMN email TEXT;
CREATE UNIQUE INDEX users_email_key ON users (lower(email)) WHERE email IS NOT NULL;

-- Um código por conta, de cada vez. Guarda só o sha256 do código; vence
-- em 15 minutos e some ao ser usado ou depois de errar demais.
CREATE TABLE password_resets (
    user_id UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    code_hash BYTEA NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    attempts INT NOT NULL DEFAULT 0
);
