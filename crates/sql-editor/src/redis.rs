//! IntelliSense do Redis: comandos (com a sintaxe ao lado), subcomandos
//! e, onde o comando espera uma chave, as chaves do banco em cache.

use pgstudio_core::catalog::Catalog;
use pgstudio_core::connection::redis_argument;

use crate::complete::{Completion, Kind, Suggestion};
use crate::lexer::{Token, TokenKind};

/// Onde um comando recebe chaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyArg {
    /// Em nenhum argumento (ou só depois de opções, como no `XREAD`).
    None,
    /// No primeiro argumento.
    First,
    /// Em todos (`DEL a b c`).
    All,
}

/// Um comando: nome, argumentos como na documentação e onde vão as
/// chaves. Em ordem alfabética.
pub const COMMANDS: &[(&str, &str, KeyArg)] = &[
    (
        "ACL",
        "LIST | USERS | WHOAMI | GETUSER user | SETUSER user [rule ...] | DELUSER user [...] | CAT [category] | LOG [count|RESET]",
        KeyArg::None,
    ),
    ("APPEND", "key value", KeyArg::First),
    ("AUTH", "[username] password", KeyArg::None),
    ("BGREWRITEAOF", "", KeyArg::None),
    ("BGSAVE", "[SCHEDULE]", KeyArg::None),
    ("BITCOUNT", "key [start end [BYTE|BIT]]", KeyArg::First),
    ("BITPOS", "key bit [start [end [BYTE|BIT]]]", KeyArg::First),
    ("BLPOP", "key [key ...] timeout", KeyArg::All),
    ("BRPOP", "key [key ...] timeout", KeyArg::All),
    (
        "CLIENT",
        "LIST | INFO | ID | GETNAME | SETNAME name | KILL ... | NO-EVICT ON|OFF",
        KeyArg::None,
    ),
    (
        "CLUSTER",
        "INFO | NODES | SLOTS | SHARDS | KEYSLOT key | MYID",
        KeyArg::None,
    ),
    (
        "COMMAND",
        "COUNT | DOCS [name ...] | INFO [name ...] | LIST",
        KeyArg::None,
    ),
    (
        "CONFIG",
        "GET parameter | SET parameter value | REWRITE | RESETSTAT",
        KeyArg::None,
    ),
    ("COPY", "source destination [DB db] [REPLACE]", KeyArg::All),
    ("DBSIZE", "", KeyArg::None),
    ("DEBUG", "OBJECT key | SLEEP seconds", KeyArg::None),
    ("DECR", "key", KeyArg::First),
    ("DECRBY", "key decrement", KeyArg::First),
    ("DEL", "key [key ...]", KeyArg::All),
    ("DISCARD", "", KeyArg::None),
    ("DUMP", "key", KeyArg::First),
    ("ECHO", "message", KeyArg::None),
    ("EVAL", "script numkeys [key ...] [arg ...]", KeyArg::None),
    ("EVALSHA", "sha1 numkeys [key ...] [arg ...]", KeyArg::None),
    ("EXEC", "", KeyArg::None),
    ("EXISTS", "key [key ...]", KeyArg::All),
    ("EXPIRE", "key seconds [NX|XX|GT|LT]", KeyArg::First),
    ("EXPIREAT", "key unix-time-seconds", KeyArg::First),
    ("EXPIRETIME", "key", KeyArg::First),
    (
        "FCALL",
        "function numkeys [key ...] [arg ...]",
        KeyArg::None,
    ),
    ("FLUSHALL", "[ASYNC|SYNC]", KeyArg::None),
    ("FLUSHDB", "[ASYNC|SYNC]", KeyArg::None),
    (
        "FUNCTION",
        "LIST | LOAD code | DELETE name | FLUSH | STATS",
        KeyArg::None,
    ),
    (
        "GEOADD",
        "key [NX|XX] [CH] longitude latitude member [...]",
        KeyArg::First,
    ),
    ("GEODIST", "key member1 member2 [M|KM|FT|MI]", KeyArg::First),
    ("GEOPOS", "key member [member ...]", KeyArg::First),
    (
        "GEOSEARCH",
        "key FROMMEMBER member|FROMLONLAT lon lat BYRADIUS r unit|BYBOX w h unit [ASC|DESC] [COUNT n] [WITHCOORD] [WITHDIST]",
        KeyArg::First,
    ),
    ("GET", "key", KeyArg::First),
    ("GETBIT", "key offset", KeyArg::First),
    ("GETDEL", "key", KeyArg::First),
    ("GETEX", "key [EX seconds|PX ms|PERSIST]", KeyArg::First),
    ("GETRANGE", "key start end", KeyArg::First),
    ("GETSET", "key value", KeyArg::First),
    ("HDEL", "key field [field ...]", KeyArg::First),
    (
        "HELLO",
        "[protover [AUTH username password] [SETNAME clientname]]",
        KeyArg::None,
    ),
    ("HEXISTS", "key field", KeyArg::First),
    ("HGET", "key field", KeyArg::First),
    ("HGETALL", "key", KeyArg::First),
    ("HINCRBY", "key field increment", KeyArg::First),
    ("HINCRBYFLOAT", "key field increment", KeyArg::First),
    ("HKEYS", "key", KeyArg::First),
    ("HLEN", "key", KeyArg::First),
    ("HMGET", "key field [field ...]", KeyArg::First),
    ("HMSET", "key field value [field value ...]", KeyArg::First),
    ("HRANDFIELD", "key [count [WITHVALUES]]", KeyArg::First),
    (
        "HSCAN",
        "key cursor [MATCH pattern] [COUNT count]",
        KeyArg::First,
    ),
    ("HSET", "key field value [field value ...]", KeyArg::First),
    ("HSETNX", "key field value", KeyArg::First),
    ("HSTRLEN", "key field", KeyArg::First),
    ("HVALS", "key", KeyArg::First),
    ("INCR", "key", KeyArg::First),
    ("INCRBY", "key increment", KeyArg::First),
    ("INCRBYFLOAT", "key increment", KeyArg::First),
    ("INFO", "[section ...]", KeyArg::None),
    ("JSON.DEL", "key [path]", KeyArg::First),
    ("JSON.GET", "key [path ...]", KeyArg::First),
    ("JSON.MGET", "key [key ...] path", KeyArg::None),
    ("JSON.SET", "key path value [NX|XX]", KeyArg::First),
    ("JSON.TYPE", "key [path]", KeyArg::First),
    ("KEYS", "pattern", KeyArg::None),
    ("LASTSAVE", "", KeyArg::None),
    (
        "LATENCY",
        "LATEST | HISTORY event | RESET [event ...] | DOCTOR",
        KeyArg::None,
    ),
    ("LINDEX", "key index", KeyArg::First),
    ("LINSERT", "key BEFORE|AFTER pivot element", KeyArg::First),
    ("LLEN", "key", KeyArg::First),
    (
        "LMOVE",
        "source destination LEFT|RIGHT LEFT|RIGHT",
        KeyArg::All,
    ),
    ("LPOP", "key [count]", KeyArg::First),
    (
        "LPOS",
        "key element [RANK rank] [COUNT num] [MAXLEN len]",
        KeyArg::First,
    ),
    ("LPUSH", "key element [element ...]", KeyArg::First),
    ("LPUSHX", "key element [element ...]", KeyArg::First),
    ("LRANGE", "key start stop", KeyArg::First),
    ("LREM", "key count element", KeyArg::First),
    ("LSET", "key index element", KeyArg::First),
    ("LTRIM", "key start stop", KeyArg::First),
    ("MEMORY", "USAGE key | STATS | DOCTOR", KeyArg::None),
    ("MGET", "key [key ...]", KeyArg::All),
    (
        "MODULE",
        "LIST | LOAD path [arg ...] | UNLOAD name",
        KeyArg::None,
    ),
    ("MONITOR", "", KeyArg::None),
    ("MOVE", "key db", KeyArg::First),
    ("MSET", "key value [key value ...]", KeyArg::First),
    ("MSETNX", "key value [key value ...]", KeyArg::First),
    ("MULTI", "", KeyArg::None),
    (
        "OBJECT",
        "ENCODING|FREQ|IDLETIME|REFCOUNT key",
        KeyArg::None,
    ),
    ("PERSIST", "key", KeyArg::First),
    ("PEXPIRE", "key milliseconds [NX|XX|GT|LT]", KeyArg::First),
    ("PFADD", "key [element ...]", KeyArg::First),
    ("PFCOUNT", "key [key ...]", KeyArg::All),
    ("PFMERGE", "destkey [sourcekey ...]", KeyArg::All),
    ("PING", "[message]", KeyArg::None),
    ("PSETEX", "key milliseconds value", KeyArg::First),
    ("PSUBSCRIBE", "pattern [pattern ...]", KeyArg::None),
    ("PTTL", "key", KeyArg::First),
    ("PUBLISH", "channel message", KeyArg::None),
    (
        "PUBSUB",
        "CHANNELS [pattern] | NUMSUB [channel ...] | NUMPAT",
        KeyArg::None,
    ),
    ("QUIT", "", KeyArg::None),
    ("RANDOMKEY", "", KeyArg::None),
    ("RENAME", "key newkey", KeyArg::All),
    ("RENAMENX", "key newkey", KeyArg::All),
    ("REPLICAOF", "host port | NO ONE", KeyArg::None),
    ("ROLE", "", KeyArg::None),
    ("RPOP", "key [count]", KeyArg::First),
    ("RPUSH", "key element [element ...]", KeyArg::First),
    ("RPUSHX", "key element [element ...]", KeyArg::First),
    ("SADD", "key member [member ...]", KeyArg::First),
    ("SAVE", "", KeyArg::None),
    (
        "SCAN",
        "cursor [MATCH pattern] [COUNT count] [TYPE type]",
        KeyArg::None,
    ),
    ("SCARD", "key", KeyArg::First),
    (
        "SCRIPT",
        "LOAD script | EXISTS sha1 [...] | FLUSH | KILL",
        KeyArg::None,
    ),
    ("SDIFF", "key [key ...]", KeyArg::All),
    ("SDIFFSTORE", "destination key [key ...]", KeyArg::All),
    ("SELECT", "index", KeyArg::None),
    (
        "SET",
        "key value [NX|XX] [GET] [EX seconds|PX ms|KEEPTTL]",
        KeyArg::First,
    ),
    ("SETBIT", "key offset value", KeyArg::First),
    ("SETEX", "key seconds value", KeyArg::First),
    ("SETNX", "key value", KeyArg::First),
    ("SETRANGE", "key offset value", KeyArg::First),
    ("SINTER", "key [key ...]", KeyArg::All),
    ("SINTERSTORE", "destination key [key ...]", KeyArg::All),
    ("SISMEMBER", "key member", KeyArg::First),
    ("SLOWLOG", "GET [count] | LEN | RESET", KeyArg::None),
    ("SMEMBERS", "key", KeyArg::First),
    ("SMISMEMBER", "key member [member ...]", KeyArg::First),
    ("SMOVE", "source destination member", KeyArg::First),
    (
        "SORT",
        "key [BY pattern] [LIMIT offset count] [ASC|DESC] [ALPHA] [STORE destination]",
        KeyArg::First,
    ),
    ("SPOP", "key [count]", KeyArg::First),
    ("SRANDMEMBER", "key [count]", KeyArg::First),
    ("SREM", "key member [member ...]", KeyArg::First),
    (
        "SSCAN",
        "key cursor [MATCH pattern] [COUNT count]",
        KeyArg::First,
    ),
    ("STRLEN", "key", KeyArg::First),
    ("SUBSCRIBE", "channel [channel ...]", KeyArg::None),
    ("SUNION", "key [key ...]", KeyArg::All),
    ("SUNIONSTORE", "destination key [key ...]", KeyArg::All),
    ("SWAPDB", "index1 index2", KeyArg::None),
    ("TIME", "", KeyArg::None),
    ("TOUCH", "key [key ...]", KeyArg::All),
    ("TTL", "key", KeyArg::First),
    ("TYPE", "key", KeyArg::First),
    ("UNLINK", "key [key ...]", KeyArg::All),
    ("UNWATCH", "", KeyArg::None),
    ("WAIT", "numreplicas timeout", KeyArg::None),
    ("WATCH", "key [key ...]", KeyArg::All),
    ("XACK", "key group id [id ...]", KeyArg::First),
    (
        "XADD",
        "key [NOMKSTREAM] [MAXLEN|MINID [=|~] threshold] *|id field value [field value ...]",
        KeyArg::First,
    ),
    ("XDEL", "key id [id ...]", KeyArg::First),
    (
        "XGROUP",
        "CREATE|DESTROY|SETID|CREATECONSUMER|DELCONSUMER key group ...",
        KeyArg::None,
    ),
    ("XINFO", "STREAM|GROUPS|CONSUMERS key [group]", KeyArg::None),
    ("XLEN", "key", KeyArg::First),
    (
        "XPENDING",
        "key group [[IDLE min-idle] start end count [consumer]]",
        KeyArg::First,
    ),
    ("XRANGE", "key start end [COUNT count]", KeyArg::First),
    (
        "XREAD",
        "[COUNT count] [BLOCK ms] STREAMS key [key ...] id [id ...]",
        KeyArg::None,
    ),
    (
        "XREADGROUP",
        "GROUP group consumer [COUNT count] [BLOCK ms] STREAMS key [key ...] id [id ...]",
        KeyArg::None,
    ),
    ("XREVRANGE", "key end start [COUNT count]", KeyArg::First),
    ("XTRIM", "key MAXLEN|MINID [=|~] threshold", KeyArg::First),
    (
        "ZADD",
        "key [NX|XX] [GT|LT] [CH] [INCR] score member [score member ...]",
        KeyArg::First,
    ),
    ("ZCARD", "key", KeyArg::First),
    ("ZCOUNT", "key min max", KeyArg::First),
    ("ZINCRBY", "key increment member", KeyArg::First),
    ("ZMSCORE", "key member [member ...]", KeyArg::First),
    ("ZPOPMAX", "key [count]", KeyArg::First),
    ("ZPOPMIN", "key [count]", KeyArg::First),
    ("ZRANDMEMBER", "key [count [WITHSCORES]]", KeyArg::First),
    (
        "ZRANGE",
        "key start stop [BYSCORE|BYLEX] [REV] [LIMIT offset count] [WITHSCORES]",
        KeyArg::First,
    ),
    (
        "ZRANGEBYSCORE",
        "key min max [WITHSCORES] [LIMIT offset count]",
        KeyArg::First,
    ),
    ("ZRANK", "key member [WITHSCORE]", KeyArg::First),
    ("ZREM", "key member [member ...]", KeyArg::First),
    ("ZREMRANGEBYRANK", "key start stop", KeyArg::First),
    ("ZREMRANGEBYSCORE", "key min max", KeyArg::First),
    ("ZREVRANGE", "key start stop [WITHSCORES]", KeyArg::First),
    (
        "ZREVRANGEBYSCORE",
        "key max min [WITHSCORES] [LIMIT offset count]",
        KeyArg::First,
    ),
    ("ZREVRANK", "key member [WITHSCORE]", KeyArg::First),
    (
        "ZSCAN",
        "key cursor [MATCH pattern] [COUNT count]",
        KeyArg::First,
    ),
    ("ZSCORE", "key member", KeyArg::First),
];

/// Comandos que têm subcomandos e quais são.
pub const SUBCOMMANDS: &[(&str, &[&str])] = &[
    (
        "ACL",
        &[
            "LIST", "USERS", "WHOAMI", "GETUSER", "SETUSER", "DELUSER", "CAT", "LOG", "GENPASS",
            "LOAD", "SAVE", "HELP",
        ],
    ),
    (
        "CLIENT",
        &[
            "LIST", "INFO", "ID", "GETNAME", "SETNAME", "KILL", "PAUSE", "UNPAUSE", "NO-EVICT",
            "TRACKING", "HELP",
        ],
    ),
    (
        "CLUSTER",
        &[
            "INFO",
            "NODES",
            "SLOTS",
            "SHARDS",
            "KEYSLOT",
            "MYID",
            "COUNTKEYSINSLOT",
            "HELP",
        ],
    ),
    (
        "COMMAND",
        &["COUNT", "DOCS", "INFO", "LIST", "GETKEYS", "HELP"],
    ),
    ("CONFIG", &["GET", "SET", "REWRITE", "RESETSTAT", "HELP"]),
    ("DEBUG", &["OBJECT", "SLEEP", "JMAP"]),
    (
        "FUNCTION",
        &[
            "LIST", "LOAD", "DELETE", "FLUSH", "STATS", "DUMP", "RESTORE", "KILL", "HELP",
        ],
    ),
    (
        "LATENCY",
        &["LATEST", "HISTORY", "RESET", "DOCTOR", "GRAPH", "HELP"],
    ),
    (
        "MEMORY",
        &["USAGE", "STATS", "DOCTOR", "PURGE", "MALLOC-STATS", "HELP"],
    ),
    ("MODULE", &["LIST", "LOAD", "UNLOAD", "HELP"]),
    (
        "OBJECT",
        &["ENCODING", "FREQ", "IDLETIME", "REFCOUNT", "HELP"],
    ),
    (
        "PUBSUB",
        &[
            "CHANNELS",
            "NUMSUB",
            "NUMPAT",
            "SHARDCHANNELS",
            "SHARDNUMSUB",
            "HELP",
        ],
    ),
    ("SCRIPT", &["LOAD", "EXISTS", "FLUSH", "KILL", "HELP"]),
    ("SLOWLOG", &["GET", "LEN", "RESET", "HELP"]),
    (
        "XGROUP",
        &[
            "CREATE",
            "DESTROY",
            "SETID",
            "CREATECONSUMER",
            "DELCONSUMER",
            "HELP",
        ],
    ),
    ("XINFO", &["STREAM", "GROUPS", "CONSUMERS", "HELP"]),
];

/// Chaves mostradas de uma vez na lista.
const MAX_KEYS: usize = 200;

fn starts_with(name: &str, prefix: &str) -> bool {
    crate::fuzzy::matches(name, prefix)
}

/// Sugestões para o cursor em `at` (bytes) num texto de comandos do
/// Redis. As chaves vêm de `catalog` (as "tabelas" do schema sem nome).
pub fn complete(text: &str, tokens: &[Token], at: usize, catalog: Option<&Catalog>) -> Completion {
    let none = Completion {
        replace: at..at,
        ..Completion::default()
    };
    let i = tokens.partition_point(|t| t.range.end < at);
    let mut start = at;
    if let Some(t) = tokens.get(i).filter(|t| t.range.start < at) {
        match t.kind {
            TokenKind::Comment | TokenKind::String | TokenKind::Number => return none,
            TokenKind::Whitespace => {}
            _ => start = t.range.start,
        }
    }
    let prefix = &text[start..at];
    // As palavras da linha antes da que está sendo escrita.
    let line = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let before: Vec<&str> = tokens
        .iter()
        .filter(|t| t.range.start >= line && t.range.end <= start)
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
        .map(|t| &text[t.range.clone()])
        .collect();
    let mut items = Vec::new();
    match before.first() {
        None => {
            if tokens.iter().any(|t| {
                t.kind == TokenKind::Comment && t.range.start >= line && t.range.end <= start
            }) {
                return none;
            }
            for (name, syntax, _) in COMMANDS {
                if starts_with(name, prefix) {
                    items.push(Suggestion {
                        label: name.to_string(),
                        insert: name.to_string(),
                        kind: Kind::Keyword,
                        detail: syntax.to_string(),
                    });
                }
            }
        }
        Some(command) => {
            let command = command.to_ascii_uppercase();
            let subs = SUBCOMMANDS
                .iter()
                .find(|(name, _)| *name == command)
                .map(|(_, subs)| *subs);
            let keys = COMMANDS
                .iter()
                .find(|(name, ..)| *name == command)
                .map_or(KeyArg::None, |(.., k)| *k);
            if let (Some(subs), 1) = (subs, before.len()) {
                for sub in subs {
                    if starts_with(sub, prefix) {
                        items.push(Suggestion {
                            label: sub.to_string(),
                            insert: sub.to_string(),
                            kind: Kind::Keyword,
                            detail: command.clone(),
                        });
                    }
                }
            } else if keys == KeyArg::All || (keys == KeyArg::First && before.len() == 1) {
                let names = catalog
                    .into_iter()
                    .flat_map(|c| &c.schemas)
                    .flat_map(|s| &s.relations)
                    .filter(|r| {
                        starts_with(&r.name, prefix) && !before[1..].contains(&r.name.as_str())
                    });
                for key in names {
                    items.push(Suggestion {
                        label: key.name.clone(),
                        insert: redis_argument(&key.name),
                        kind: Kind::Table,
                        // O tipo da chave, guardado como a coluna dela.
                        detail: key
                            .columns
                            .first()
                            .map(|c| c.ty.clone())
                            .unwrap_or_default(),
                    });
                }
            }
        }
    }
    // Os que começam igual primeiro, os aproximados depois.
    crate::fuzzy::sort(&mut items, prefix);
    items.truncate(MAX_KEYS.max(COMMANDS.len()));
    Completion {
        replace: start..at,
        prefix: prefix.to_string(),
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{Dialect, tokenize_in};
    use pgstudio_core::catalog::{Column, Relation, RelationKind, Schema};
    use pgstudio_core::connection::Engine;

    fn catalog(keys: &[(&str, &str)]) -> Catalog {
        Catalog {
            engine: Engine::Redis,
            database: "db0".into(),
            search_path: vec![String::new()],
            schemas: vec![Schema {
                name: String::new(),
                relations: keys
                    .iter()
                    .map(|(name, ty)| Relation {
                        name: name.to_string(),
                        kind: RelationKind::Table,
                        columns: vec![Column {
                            name: "value".into(),
                            ty: ty.to_string(),
                        }],
                    })
                    .collect(),
                functions: Vec::new(),
            }],
        }
    }

    /// As sugestões com o cursor no `|` de `text`.
    fn at(text: &str, catalog: Option<&Catalog>) -> (Vec<String>, Completion) {
        let pos = text.find('|').unwrap();
        let text = text.replace('|', "");
        let tokens = tokenize_in(&text, Dialect::Redis);
        let c = complete(&text, &tokens, pos, catalog);
        (c.items.iter().map(|s| s.label.clone()).collect(), c)
    }

    #[test]
    fn commands_are_sorted_and_unique() {
        assert!(COMMANDS.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(SUBCOMMANDS.windows(2).all(|w| w[0].0 < w[1].0));
        for (name, _) in SUBCOMMANDS {
            assert!(COMMANDS.iter().any(|(c, ..)| c == name), "{name}");
        }
    }

    #[test]
    fn first_word_suggests_commands_with_syntax() {
        let (labels, c) = at("SET a 1\nhge|", None);
        assert_eq!(labels, ["HGET", "HGETALL"]);
        assert_eq!(c.items[0].detail, "key field");
        assert_eq!(c.replace, 8..11);
        let (labels, _) = at("|", None);
        assert_eq!(labels.len(), COMMANDS.len());
        // Comentário e texto entre aspas: nada.
        assert!(at("# hge|", None).0.is_empty());
        assert!(at("SET k \"hge|", None).0.is_empty());
    }

    #[test]
    fn subcommands_follow_their_command() {
        let (labels, _) = at("config |", None);
        assert_eq!(labels, ["GET", "SET", "REWRITE", "RESETSTAT", "HELP"]);
        let (labels, _) = at("CLIENT se|", None);
        assert_eq!(labels, ["SETNAME"]);
        assert!(at("CONFIG GET |", None).0.is_empty());
    }

    #[test]
    fn keys_come_from_the_catalog() {
        let cat = catalog(&[("user:1", "hash"), ("user:2", "hash"), ("my key", "string")]);
        let (labels, c) = at("HGETALL us|", Some(&cat));
        assert_eq!(labels, ["user:1", "user:2"]);
        assert_eq!(c.items[0].detail, "hash");
        assert_eq!(c.prefix, "us");
        // Só no primeiro argumento do HGET; em todos do DEL, sem repetir.
        assert!(at("HGET user:1 |", Some(&cat)).0.is_empty());
        let (labels, c) = at("DEL user:1 |", Some(&cat));
        assert_eq!(labels, ["user:2", "my key"]);
        assert_eq!(c.items[1].insert, "\"my key\"");
        // Sem catálogo (ainda carregando) e em comandos sem chave: nada.
        assert!(at("GET |", None).0.is_empty());
        assert!(at("PUBLISH |", Some(&cat)).0.is_empty());
    }
}
