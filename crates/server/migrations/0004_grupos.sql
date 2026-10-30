-- Grupos criados pelas pessoas têm dono. A Sala do Grupo fica sem dono.
-- Se a conta do dono sumir, o grupo continua, sem dono.
ALTER TABLE conversations ADD COLUMN owner_id UUID REFERENCES users (id) ON DELETE SET NULL;
