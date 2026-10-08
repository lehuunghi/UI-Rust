INSERT INTO server_capacity(server_id) SELECT id FROM stalwart_servers ON CONFLICT DO NOTHING;
CREATE FUNCTION rust_seed_capacity() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO server_capacity(server_id) VALUES(NEW.id) ON CONFLICT DO NOTHING; RETURN NEW; END $$;
CREATE TRIGGER rust_server_capacity AFTER INSERT ON stalwart_servers FOR EACH ROW EXECUTE FUNCTION rust_seed_capacity();
