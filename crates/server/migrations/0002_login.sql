-- Login de verdade: toda conta passa a ter senha.
-- As contas sem senha eram provisórias (só um nome) e saem junto com suas mensagens.
DELETE FROM users WHERE password_hash IS NULL;
ALTER TABLE users ALTER COLUMN password_hash SET NOT NULL;
