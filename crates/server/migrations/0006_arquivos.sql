-- Áudios continuam com duração; imagens e outros arquivos têm nome e tamanho.
ALTER TABLE attachments ADD COLUMN file_name TEXT;
