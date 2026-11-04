-- Editar e apagar mensagens. Apagar é DELETE mesmo: some para todo mundo.
-- Editar guarda quando foi, para a tela marcar "editada".
ALTER TABLE messages ADD COLUMN edited_at TIMESTAMPTZ;
