-- Login de verdade: senha nas contas e sessões com token.

-- As contas da fase sem senha não têm como entrar, então saem junto com o
-- que escreveram. Só existiam em bancos de desenvolvimento.
TRUNCATE users CASCADE;

-- Hash Argon2 no formato PHC ("$argon2id$..."), nunca a senha em si.
ALTER TABLE users ADD COLUMN password_hash TEXT NOT NULL;

-- O nome é guardado sempre em minúsculas, então "Nina" e "nina" são a mesma conta.
ALTER TABLE users ADD CONSTRAINT username_lowercase CHECK (username = lower(username));

-- Uma linha por aparelho logado. Guarda só o SHA-256 do token: quem ler o
-- banco não consegue entrar na conta de ninguém.
CREATE TABLE sessions (
    token_hash   BYTEA PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX sessions_by_user ON sessions (user_id);
