-- The tables of format 4. Never change this text.
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS accounts (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    kind TEXT NOT NULL,
    currency TEXT NOT NULL DEFAULT 'EUR',
    iban TEXT NOT NULL DEFAULT '',
    archived INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS categories (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    parent_id INTEGER REFERENCES categories(id) ON DELETE CASCADE,
    is_income INTEGER NOT NULL DEFAULT 0,
    UNIQUE(name, parent_id)
);
CREATE TABLE IF NOT EXISTS category_budgets (
    category_id INTEGER PRIMARY KEY REFERENCES categories(id) ON DELETE CASCADE,
    monthly INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS category_rules (
    id INTEGER PRIMARY KEY,
    pattern TEXT NOT NULL,
    category_id INTEGER NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
    rename_payee TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS scheduled_items (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    to_account_id INTEGER REFERENCES accounts(id) ON DELETE CASCADE,
    amount INTEGER NOT NULL,
    payee TEXT NOT NULL DEFAULT '',
    memo TEXT NOT NULL DEFAULT '',
    category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    next_date TEXT NOT NULL,
    every INTEGER NOT NULL DEFAULT 1,
    unit TEXT NOT NULL DEFAULT 'month'
);
CREATE TABLE ledger_accounts (
    id INTEGER PRIMARY KEY,
    class TEXT NOT NULL CHECK (class IN ('asset','liability','equity','income','expense')),
    parent_id INTEGER REFERENCES ledger_accounts(id) ON DELETE SET NULL,
    name TEXT NOT NULL DEFAULT '',
    code TEXT NOT NULL DEFAULT '',
    currency TEXT,
    money_account_id INTEGER UNIQUE REFERENCES accounts(id),
    category_id INTEGER UNIQUE REFERENCES categories(id),
    system TEXT UNIQUE
);
CREATE INDEX ledger_parent ON ledger_accounts(parent_id);
CREATE UNIQUE INDEX ledger_code ON ledger_accounts(code) WHERE code <> '';
CREATE TABLE entries (
    id INTEGER PRIMARY KEY,
    date TEXT NOT NULL,
    payee TEXT NOT NULL DEFAULT '',
    memo TEXT NOT NULL DEFAULT '',
    opening_account_id INTEGER UNIQUE REFERENCES accounts(id)
);
CREATE INDEX entry_date ON entries(date);
CREATE TABLE postings (
    id INTEGER PRIMARY KEY,
    entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    ledger_account_id INTEGER NOT NULL REFERENCES ledger_accounts(id),
    amount INTEGER NOT NULL CHECK (typeof(amount) = 'integer'),
    currency TEXT NOT NULL,
    cleared INTEGER NOT NULL DEFAULT 0,
    import_hash TEXT,
    payee TEXT,
    memo TEXT,
    date TEXT
);
CREATE INDEX posting_entry ON postings(entry_id);
CREATE INDEX posting_account ON postings(ledger_account_id);
CREATE UNIQUE INDEX posting_import_hash ON postings(ledger_account_id, import_hash) WHERE import_hash IS NOT NULL;
CREATE TABLE ledger_turnover (id INTEGER PRIMARY KEY CHECK (id = 1), total INTEGER NOT NULL);
INSERT INTO ledger_turnover VALUES (1, 0);
CREATE TRIGGER posting_added AFTER INSERT ON postings BEGIN
    UPDATE ledger_turnover SET total = total + abs(NEW.amount);
END;
CREATE TRIGGER posting_changed AFTER UPDATE OF amount ON postings BEGIN
    UPDATE ledger_turnover SET total = total - abs(OLD.amount) + abs(NEW.amount);
END;
CREATE TRIGGER posting_removed AFTER DELETE ON postings BEGIN
    UPDATE ledger_turnover SET total = total - abs(OLD.amount);
END;
CREATE TRIGGER account_gets_ledger AFTER INSERT ON accounts BEGIN
    INSERT INTO ledger_accounts(class, currency, money_account_id)
    VALUES (CASE WHEN NEW.kind IN ('credit_card','loan') THEN 'liability' ELSE 'asset' END,
            CASE WHEN trim(NEW.currency) = '' THEN 'EUR' ELSE upper(trim(NEW.currency)) END, NEW.id);
END;
CREATE TRIGGER account_kind_sets_class AFTER UPDATE OF kind ON accounts BEGIN
    UPDATE ledger_accounts
    SET class = CASE WHEN NEW.kind IN ('credit_card','loan') THEN 'liability' ELSE 'asset' END
    WHERE money_account_id = NEW.id;
END;
CREATE TRIGGER category_gets_ledger AFTER INSERT ON categories BEGIN
    INSERT INTO ledger_accounts(class, parent_id, category_id)
    VALUES (CASE WHEN NEW.is_income THEN 'income' ELSE 'expense' END,
            (SELECT id FROM ledger_accounts WHERE category_id = NEW.parent_id), NEW.id);
END;
INSERT INTO ledger_accounts(class, system) VALUES ('equity', 'opening_balances'), ('expense', 'uncategorized');
CREATE TABLE card_statements (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    closing_date TEXT NOT NULL,
    debit_date TEXT NOT NULL,
    amount INTEGER NOT NULL,
    import_key TEXT NOT NULL,
    payment_id INTEGER REFERENCES postings(id) ON DELETE SET NULL,
    payment_joined INTEGER NOT NULL DEFAULT 0,
    UNIQUE (account_id, import_key)
);
CREATE INDEX card_statement_payment ON card_statements(payment_id);
