#!/usr/bin/env python3
"""Exercise the actual shipped schema using only the Python standard library.

This does not compile Rust or claim to run the adapter/CLI integration tests.
"""
import pathlib
import sqlite3
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


class SchemaTests(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(":memory:")
        self.db.execute("PRAGMA foreign_keys=ON")
        self.db.executescript((ROOT / "crates/index/src/schema.sql").read_text())
        self.db.execute("INSERT INTO sources VALUES('src','pi','/source','/source/p.jsonl','f',1,1,0)")
        self.db.execute("""INSERT INTO sessions VALUES(
            'pi:one','one','pi','src','Title','/project',100,200,'/source/p.jsonl',NULL,'[]',1,0)""")
        self.db.commit()

    def tearDown(self):
        self.db.close()

    def insert(self, text, ordinal=1):
        self.db.execute("INSERT INTO documents(session_key,ordinal,body) VALUES('pi:one',?,?)", (ordinal, text))

    def match(self, text):
        phrase = '"' + text.replace('"', '""') + '"'
        return self.db.execute("""SELECT d.ordinal,d.body FROM documents_fts
            JOIN documents d ON d.id=documents_fts.rowid
            WHERE documents_fts MATCH ? ORDER BY bm25(documents_fts),d.id""", (phrase,)).fetchall()

    def test_trigram_unicode_and_ascii(self):
        self.insert("Buffer Management 修复渲染问题")
        self.assertEqual(len(self.match("buffer man")), 1)
        self.assertEqual(len(self.match("渲染问题")), 1)

    def test_literal_queries_do_not_become_fts_syntax(self):
        self.insert("SELECT literal \"quoted\" OR text")
        self.assertEqual(len(self.match('"quoted"')), 1)
        self.assertEqual(self.match("' OR 1=1; --"), [])
        self.assertEqual(self.match("OR *"), [])

    def test_short_search_and_filters(self):
        self.insert("中文 渲染 Buffer")
        rows = self.db.execute("""SELECT d.body FROM documents d
            JOIN sessions s ON s.key=d.session_key
            WHERE instr(lower(d.body),lower(?))>0 AND s.agent=?
            AND s.project_path=? AND s.updated_at>=? AND s.updated_at<?
            AND s.available=1 LIMIT ? OFFSET ?""", ("渲染", "pi", "/project", 200, 201, 50, 0)).fetchall()
        self.assertEqual(len(rows), 1)

    def test_delete_update_triggers_remove_old_terms(self):
        self.insert("obsolete phrase")
        self.db.execute("UPDATE documents SET body='replacement phrase'")
        self.assertEqual(self.match("obsolete"), [])
        self.assertEqual(len(self.match("replacement")), 1)
        self.db.execute("DELETE FROM documents WHERE session_key='pi:one'")
        self.assertEqual(self.match("replacement"), [])

    def test_rollback_restores_old_search_and_metadata(self):
        self.insert("known good phrase")
        self.db.commit()
        self.db.execute("BEGIN IMMEDIATE")
        self.db.execute("UPDATE sessions SET available=0,title='bad'")
        self.db.execute("DELETE FROM documents")
        self.insert("uncommitted phrase")
        self.db.rollback()
        self.assertEqual(len(self.match("known good")), 1)
        self.assertEqual(self.match("uncommitted"), [])
        self.assertEqual(self.db.execute("SELECT available,title FROM sessions").fetchone(), (1, "Title"))

    def test_uniqueness_and_foreign_keys(self):
        self.insert("unique")
        with self.assertRaises(sqlite3.IntegrityError):
            self.insert("duplicate")
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute("INSERT INTO documents(session_key,ordinal,body) VALUES('missing',2,'invalid')")

    def test_integrity_and_query_indexes(self):
        self.insert("verify database integrity")
        self.db.execute("INSERT INTO documents_fts(documents_fts,rank) VALUES('integrity-check',1)")
        self.assertEqual(self.db.execute("PRAGMA integrity_check").fetchone(), ("ok",))
        indexes = {r[1] for r in self.db.execute("PRAGMA index_list(sessions)")}
        self.assertTrue({"sessions_updated", "sessions_agent_updated", "sessions_project_updated", "sessions_source"} <= indexes)

    def test_shipped_rust_search_statement(self):
        import re
        source = (ROOT / "crates/index/src/lib.rs").read_text()
        where = re.search(r'const WHERE_FILTER: &str = "([^"]+)";', source).group(1)
        self.insert("Buffer Management 修复渲染问题")
        # Same bound values and joins used by Store::search.
        for long, query in [(True, "渲染问题"), (False, "渲染")]:
            if long:
                table = "documents_fts JOIN documents d ON d.id=documents_fts.rowid JOIN sessions s ON s.key=d.session_key"
                predicate = "documents_fts MATCH ?8"
                excerpt = "snippet(documents_fts,0,'[',']',' … ',32)"
                order = "bm25(documents_fts),s.updated_at DESC,d.id"
                query = '"' + query + '"'
            else:
                table = "documents d JOIN sessions s ON s.key=d.session_key"
                predicate = "instr(lower(d.body),lower(?8))>0"
                excerpt = "substr(d.body,max(1,instr(lower(d.body),lower(?8))-60),180)"
                order = "s.updated_at DESC,d.id"
            sql = f"SELECT s.key,s.agent,s.title,d.ordinal,{excerpt} FROM {table} WHERE {where} AND {predicate} ORDER BY {order} LIMIT ?6 OFFSET ?7"
            rows = self.db.execute(sql, ("pi", "/project", 200, 201, 0, 50, 0, query)).fetchall()
            self.assertEqual(len(rows), 1)
            self.assertEqual(rows[0][0], "pi:one")


if __name__ == "__main__":
    print(f"SQLite {sqlite3.sqlite_version}; validating shipped SQL only", flush=True)
    unittest.main(verbosity=2)
