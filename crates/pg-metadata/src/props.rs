//! Propriedades dos objetos para a janela Properties (F4).
//!
//! Cada tipo de objeto tem uma consulta de uma linha só; o nome de cada
//! coluna (o `AS "..."`) é o rótulo da propriedade e o valor vem em texto.
//! Valores nulos aparecem vazios.

use pgstudio_core::metadata::{MetadataError, NodeId, Property};
use tokio_postgres::types::ToSql;

use crate::CatalogExplorer;
use crate::path::dec;
use crate::script::{Object, object};

mod sql {
    pub const DATABASE: &str = "SELECT pg_catalog.pg_get_userbyid(d.datdba) AS \"Owner\", \
        pg_catalog.pg_encoding_to_char(d.encoding) AS \"Encoding\", \
        d.datcollate::text AS \"Collation\", \
        pg_catalog.pg_size_pretty(pg_catalog.pg_database_size(d.oid)) AS \"Size\", \
        d.datconnlimit::text AS \"Connection limit\", \
        pg_catalog.shobj_description(d.oid, 'pg_database') AS \"Description\" \
        FROM pg_catalog.pg_database d WHERE d.datname = $1";

    pub const SCHEMA: &str = "SELECT pg_catalog.pg_get_userbyid(n.nspowner) AS \"Owner\", \
        pg_catalog.obj_description(n.oid, 'pg_namespace') AS \"Description\" \
        FROM pg_catalog.pg_namespace n WHERE n.nspname = $1";

    pub const RELATION: &str = "SELECT n.nspname::text AS \"Schema\", \
        pg_catalog.pg_get_userbyid(c.relowner) AS \"Owner\", \
        CASE c.relkind WHEN 'r' THEN 'table' WHEN 'p' THEN 'partitioned table' \
            WHEN 'f' THEN 'foreign table' WHEN 'v' THEN 'view' \
            WHEN 'm' THEN 'materialized view' ELSE c.relkind::text END AS \"Kind\", \
        CASE WHEN c.relkind = 'm' THEN CASE WHEN c.relispopulated THEN 'yes' ELSE 'no' END \
            END AS \"Populated\", \
        CASE WHEN c.relkind IN ('r', 'p', 'm') THEN c.reltuples::bigint::text END \
            AS \"Rows (estimate)\", \
        CASE WHEN c.relkind IN ('r', 'p', 'm') \
            THEN pg_catalog.pg_size_pretty(pg_catalog.pg_total_relation_size(c.oid)) END \
            AS \"Size\", \
        pg_catalog.obj_description(c.oid, 'pg_class') AS \"Description\" \
        FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
        WHERE c.oid = $1";

    pub const FUNCTION: &str = "SELECT n.nspname::text AS \"Schema\", \
        pg_catalog.pg_get_userbyid(p.proowner) AS \"Owner\", \
        l.lanname::text AS \"Language\", \
        pg_catalog.pg_get_function_arguments(p.oid) AS \"Arguments\", \
        CASE WHEN p.prokind <> 'p' THEN pg_catalog.pg_get_function_result(p.oid) END \
            AS \"Returns\", \
        CASE WHEN p.prokind <> 'p' THEN CASE p.provolatile WHEN 'i' THEN 'immutable' \
            WHEN 's' THEN 'stable' ELSE 'volatile' END END AS \"Volatility\", \
        CASE WHEN p.prosecdef THEN 'definer' ELSE 'invoker' END AS \"Security\", \
        pg_catalog.obj_description(p.oid, 'pg_proc') AS \"Description\" \
        FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
        JOIN pg_catalog.pg_language l ON l.oid = p.prolang WHERE p.oid = $1";

    pub const SEQUENCE: &str = "SELECT n.nspname::text AS \"Schema\", \
        pg_catalog.pg_get_userbyid(c.relowner) AS \"Owner\", \
        pg_catalog.format_type(s.seqtypid, NULL) AS \"Data type\", \
        s.seqstart::text AS \"Start\", s.seqincrement::text AS \"Increment\", \
        s.seqmin::text AS \"Minimum\", s.seqmax::text AS \"Maximum\", \
        s.seqcache::text AS \"Cache\", \
        CASE WHEN s.seqcycle THEN 'yes' ELSE 'no' END AS \"Cycle\" \
        FROM pg_catalog.pg_sequence s JOIN pg_catalog.pg_class c ON c.oid = s.seqrelid \
        JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE s.seqrelid = $1";

    pub const INDEX: &str = "SELECT t.relname::text AS \"Table\", \
        (SELECT string_agg(pg_catalog.pg_get_indexdef(i.indexrelid, k, true) \
                || CASE WHEN i.indoption[k - 1] & 1 = 1 THEN ' DESC' ELSE '' END, ', ' ORDER BY k) \
            FROM generate_series(1, i.indnkeyatts) k) AS \"Columns\", \
        (SELECT string_agg(pg_catalog.pg_get_indexdef(i.indexrelid, k, true), ', ' ORDER BY k) \
            FROM generate_series(i.indnkeyatts + 1, i.indnatts) k) AS \"Include\", \
        pg_catalog.pg_get_expr(i.indpred, i.indrelid, true) AS \"Condition\", \
        am.amname::text AS \"Method\", \
        CASE WHEN i.indisprimary THEN 'yes' ELSE 'no' END AS \"Primary key\", \
        CASE WHEN i.indisunique THEN 'yes' ELSE 'no' END AS \"Unique\", \
        pg_catalog.pg_size_pretty(pg_catalog.pg_relation_size(c.oid)) AS \"Size\", \
        pg_catalog.pg_stat_get_numscans(c.oid)::text AS \"Scans\", \
        pg_catalog.pg_get_indexdef(c.oid) AS \"Definition\" \
        FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid = i.indexrelid \
        JOIN pg_catalog.pg_class t ON t.oid = i.indrelid \
        JOIN pg_catalog.pg_am am ON am.oid = c.relam WHERE c.oid = $1";

    pub const CONSTRAINT: &str = "SELECT t.relname::text AS \"Table\", \
        CASE k.contype WHEN 'p' THEN 'primary key' WHEN 'f' THEN 'foreign key' \
            WHEN 'u' THEN 'unique' WHEN 'c' THEN 'check' WHEN 'x' THEN 'exclusion' \
            ELSE k.contype::text END AS \"Kind\", \
        CASE WHEN k.convalidated THEN 'yes' ELSE 'no' END AS \"Validated\", \
        pg_catalog.pg_get_constraintdef(k.oid, true) AS \"Definition\" \
        FROM pg_catalog.pg_constraint k JOIN pg_catalog.pg_class t ON t.oid = k.conrelid \
        WHERE k.oid = $1";

    pub const TRIGGER: &str = "SELECT c.relname::text AS \"Table\", \
        CASE WHEN t.tgtype & 2 <> 0 THEN 'BEFORE' WHEN t.tgtype & 64 <> 0 THEN 'INSTEAD OF' \
            ELSE 'AFTER' END AS \"Timing\", \
        concat_ws(' OR ', CASE WHEN t.tgtype & 4 <> 0 THEN 'INSERT' END, \
            CASE WHEN t.tgtype & 16 <> 0 THEN 'UPDATE' END, \
            CASE WHEN t.tgtype & 8 <> 0 THEN 'DELETE' END, \
            CASE WHEN t.tgtype & 32 <> 0 THEN 'TRUNCATE' END) AS \"Events\", \
        CASE WHEN t.tgtype & 1 <> 0 THEN 'row' ELSE 'statement' END AS \"Level\", \
        t.tgfoid::regproc::text AS \"Function\", \
        CASE t.tgenabled WHEN 'D' THEN 'no' WHEN 'R' THEN 'replica only' \
            WHEN 'A' THEN 'always' ELSE 'yes' END AS \"Enabled\", \
        pg_catalog.pg_get_triggerdef(t.oid, true) AS \"Definition\" \
        FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid \
        WHERE t.oid = $1";

    pub const TYPE: &str = "SELECT n.nspname::text AS \"Schema\", \
        pg_catalog.pg_get_userbyid(t.typowner) AS \"Owner\", \
        CASE t.typtype WHEN 'e' THEN 'enum' WHEN 'd' THEN 'domain' WHEN 'c' THEN 'composite' \
            WHEN 'r' THEN 'range' WHEN 'm' THEN 'multirange' ELSE 'base' END AS \"Kind\", \
        CASE WHEN t.typtype = 'e' THEN (SELECT string_agg(e.enumlabel, ', ' \
            ORDER BY e.enumsortorder) FROM pg_catalog.pg_enum e WHERE e.enumtypid = t.oid) \
            END AS \"Values\", \
        CASE WHEN t.typtype = 'd' THEN pg_catalog.format_type(t.typbasetype, t.typtypmod) \
            WHEN t.typtype = 'r' THEN (SELECT pg_catalog.format_type(r.rngsubtype, NULL) \
                FROM pg_catalog.pg_range r WHERE r.rngtypid = t.oid) END AS \"Base type\", \
        CASE WHEN t.typtype = 'c' THEN (SELECT string_agg(a.attname || ' ' \
            || pg_catalog.format_type(a.atttypid, a.atttypmod), ', ' ORDER BY a.attnum) \
            FROM pg_catalog.pg_attribute a WHERE a.attrelid = t.typrelid AND a.attnum > 0 \
            AND NOT a.attisdropped) END AS \"Attributes\", \
        CASE WHEN t.typtype = 'd' THEN CASE WHEN t.typnotnull THEN 'no' ELSE 'yes' END END \
            AS \"Nullable\", \
        t.typdefault AS \"Default\", \
        CASE WHEN t.typtype = 'd' THEN (SELECT string_agg( \
            pg_catalog.pg_get_constraintdef(k.oid, true), ' AND ' ORDER BY k.conname) \
            FROM pg_catalog.pg_constraint k WHERE k.contypid = t.oid AND k.contype = 'c') \
            END AS \"Checks\", \
        pg_catalog.obj_description(t.oid, 'pg_type') AS \"Description\" \
        FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
        WHERE t.oid = $1";

    pub const EXTENSION: &str = "SELECT n.nspname::text AS \"Schema\", \
        e.extversion AS \"Version\", \
        (SELECT a.default_version FROM pg_catalog.pg_available_extensions a \
            WHERE a.name = e.extname) AS \"Default version\", \
        pg_catalog.pg_get_userbyid(e.extowner) AS \"Owner\", \
        CASE WHEN e.extrelocatable THEN 'yes' ELSE 'no' END AS \"Relocatable\", \
        pg_catalog.obj_description(e.oid, 'pg_extension') AS \"Description\" \
        FROM pg_catalog.pg_extension e JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace \
        WHERE e.oid = $1";

    pub const COLUMN: &str = "SELECT c.relname::text AS \"Table\", \
        pg_catalog.format_type(a.atttypid, a.atttypmod) AS \"Data type\", \
        CASE WHEN a.attnotnull THEN 'no' ELSE 'yes' END AS \"Nullable\", \
        pg_catalog.pg_get_expr(d.adbin, d.adrelid) AS \"Default\", \
        CASE a.attidentity WHEN 'a' THEN 'always' WHEN 'd' THEN 'by default' END \
            AS \"Identity\", \
        pg_catalog.col_description(a.attrelid, a.attnum) AS \"Description\" \
        FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
        LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
        WHERE a.attrelid = $1 AND a.attnum = $2";

    pub const ROLE: &str = "SELECT CASE WHEN r.rolcanlogin THEN 'yes' ELSE 'no' END AS \"Login\", \
        CASE WHEN r.rolsuper THEN 'yes' ELSE 'no' END AS \"Superuser\", \
        CASE WHEN r.rolcreatedb THEN 'yes' ELSE 'no' END AS \"Create databases\", \
        CASE WHEN r.rolcreaterole THEN 'yes' ELSE 'no' END AS \"Create roles\", \
        r.rolconnlimit::text AS \"Connection limit\", \
        r.rolvaliduntil::text AS \"Valid until\", \
        (SELECT string_agg(g.rolname, ', ' ORDER BY g.rolname) \
            FROM pg_catalog.pg_auth_members m JOIN pg_catalog.pg_roles g ON g.oid = m.roleid \
            WHERE m.member = r.oid) AS \"Member of\" \
        FROM pg_catalog.pg_roles r WHERE r.oid = $1";
}

impl CatalogExplorer {
    /// Propriedades de um nó; vazio para pastas e o servidor.
    pub(crate) fn properties_for(&self, node: &NodeId) -> Result<Vec<Property>, MetadataError> {
        if let Some(oid) = node.0.strip_prefix("role/") {
            let Ok(oid) = oid.parse::<u32>() else {
                return Ok(Vec::new());
            };
            let home = self.settings.database.clone();
            return self.property_row(&home, sql::ROLE, &[&oid], "role");
        }
        if let ["db", db, "col", rel, attnum] = node.0.split('/').collect::<Vec<_>>().as_slice() {
            let (Ok(rel), Ok(attnum)) = (rel.parse::<u32>(), attnum.parse::<i16>()) else {
                return Ok(Vec::new());
            };
            return self.property_row(&dec(db), sql::COLUMN, &[&rel, &attnum], "column");
        }
        let Some(object) = object(node) else {
            return Ok(Vec::new());
        };
        match object {
            Object::Database(db) => {
                let home = self.settings.database.clone();
                self.property_row(&home, sql::DATABASE, &[&db], "database")
            }
            Object::Schema(db, s) => self.property_row(&db, sql::SCHEMA, &[&s], "schema"),
            Object::Relation(db, oid) => self.property_row(&db, sql::RELATION, &[&oid], "table"),
            Object::Function(db, oid) => self.property_row(&db, sql::FUNCTION, &[&oid], "function"),
            Object::Sequence(db, oid) => self.property_row(&db, sql::SEQUENCE, &[&oid], "sequence"),
            Object::Index(db, oid) => self.property_row(&db, sql::INDEX, &[&oid], "index"),
            Object::Constraint(db, oid) => {
                self.property_row(&db, sql::CONSTRAINT, &[&oid], "constraint")
            }
            Object::Trigger(db, oid) => self.property_row(&db, sql::TRIGGER, &[&oid], "trigger"),
            Object::Type(db, oid) => self.property_row(&db, sql::TYPE, &[&oid], "type"),
            Object::Extension(db, oid) => {
                self.property_row(&db, sql::EXTENSION, &[&oid], "extension")
            }
        }
    }

    /// A linha de `sql` como propriedades: cada coluna é um par de nome e
    /// valor. Colunas nulas ficam de fora (não se aplicam ao objeto), a
    /// não ser a descrição, que aparece vazia como no SSMS.
    fn property_row(
        &self,
        db: &str,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
        what: &str,
    ) -> Result<Vec<Property>, MetadataError> {
        let row = self
            .query(db, sql, params)?
            .into_iter()
            .next()
            .ok_or_else(|| {
                MetadataError(format!(
                    "The {what} no longer exists. Refresh the Object Explorer."
                ))
            })?;
        Ok(row
            .columns()
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let value: Option<String> = row.get(i);
                match value {
                    Some(v) => Some(Property::new(c.name(), v)),
                    None if c.name() == "Description" => Some(Property::new(c.name(), "")),
                    None => None,
                }
            })
            .collect())
    }
}
