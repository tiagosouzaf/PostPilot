//! O texto da janela de consulta como comandos do Redis.
//!
//! Um comando por linha, com os argumentos separados por espaço e as aspas
//! do redis-cli: `"..."` com escapes (`\n`, `\t`, `\xHH`, `\"`) e `'...'`
//! (só `\'`). Um texto entre aspas pode passar de uma linha. Linhas que
//! começam com `#`, `--` ou `//` são comentários.

/// Um comando do texto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub args: Vec<Vec<u8>>,
    /// Onde o comando começa no texto, em caracteres a partir de 0.
    pub start: usize,
}

impl Command {
    /// O nome em maiúsculas (`GET`).
    pub fn name(&self) -> String {
        self.word(0)
    }

    /// O subcomando em maiúsculas (`GET` de `CONFIG GET`), ou vazio.
    pub fn sub(&self) -> String {
        self.word(1)
    }

    fn word(&self, i: usize) -> String {
        self.args
            .get(i)
            .map(|a| String::from_utf8_lossy(a).to_ascii_uppercase())
            .unwrap_or_default()
    }

    /// Nome para mensagens: `CONFIG SET` nos comandos com subcomando.
    pub fn label(&self) -> String {
        let name = self.name();
        if CONTAINERS.contains(&name.as_str()) && self.args.len() > 1 {
            format!("{name} {}", self.sub())
        } else {
            name
        }
    }

    /// O argumento `i` como texto.
    pub fn text(&self, i: usize) -> Option<String> {
        self.args
            .get(i)
            .map(|a| String::from_utf8_lossy(a).into_owned())
    }

    /// Se o comando é do cliente (`POSTPILOT ...`), que não vai ao
    /// servidor.
    pub fn is_client(&self) -> bool {
        self.name() == "POSTPILOT"
    }

    /// Se a resposta não acaba: o servidor segue mandando mensagens até a
    /// conexão fechar.
    pub fn is_stream(&self) -> bool {
        matches!(
            self.name().as_str(),
            "SUBSCRIBE" | "PSUBSCRIBE" | "SSUBSCRIBE" | "MONITOR"
        )
    }

    /// Se o comando só lê.
    pub fn is_read_only(&self) -> bool {
        let name = self.name();
        if self.is_client() || READ_ONLY.binary_search(&name.as_str()).is_ok() {
            return true;
        }
        let sub = self.sub();
        let subs: &[&str] = match name.as_str() {
            "CONFIG" => &["GET", "HELP"],
            "CLIENT" => &[
                "LIST",
                "INFO",
                "ID",
                "GETNAME",
                "HELP",
                "TRACKINGINFO",
                "GETREDIR",
            ],
            "ACL" => &[
                "LIST", "USERS", "WHOAMI", "GETUSER", "CAT", "HELP", "DRYRUN",
            ],
            "SLOWLOG" => &["GET", "LEN", "HELP"],
            "LATENCY" => &["LATEST", "HISTORY", "DOCTOR", "GRAPH", "HISTOGRAM", "HELP"],
            "MEMORY" => &["USAGE", "STATS", "DOCTOR", "MALLOC-STATS", "HELP"],
            "OBJECT" => &["ENCODING", "FREQ", "IDLETIME", "REFCOUNT", "HELP"],
            "XINFO" => &["STREAM", "GROUPS", "CONSUMERS", "HELP"],
            "CLUSTER" => &[
                "INFO",
                "NODES",
                "SLOTS",
                "SHARDS",
                "MYID",
                "MYSHARDID",
                "KEYSLOT",
                "COUNTKEYSINSLOT",
                "GETKEYSINSLOT",
                "REPLICAS",
                "LINKS",
                "HELP",
            ],
            "SCRIPT" => &["EXISTS", "HELP"],
            "FUNCTION" => &["LIST", "STATS", "DUMP", "HELP"],
            "MODULE" => &["LIST", "HELP"],
            "PUBSUB" | "COMMAND" => return true,
            _ => return false,
        };
        subs.contains(&sub.as_str())
    }
}

/// Comandos com subcomando.
const CONTAINERS: &[&str] = &[
    "ACL", "CLIENT", "CLUSTER", "COMMAND", "CONFIG", "DEBUG", "FUNCTION", "LATENCY", "MEMORY",
    "MODULE", "OBJECT", "PUBSUB", "SCRIPT", "SLOWLOG", "XGROUP", "XINFO",
];

/// Comandos que só leem, em ordem alfabética (busca binária). `SELECT`
/// entra: só troca o banco da sessão.
const READ_ONLY: &[&str] = &[
    "BITCOUNT",
    "BITPOS",
    "DBSIZE",
    "DUMP",
    "ECHO",
    "EVALSHA_RO",
    "EVAL_RO",
    "EXISTS",
    "EXPIRETIME",
    "FCALL_RO",
    "FT.AGGREGATE",
    "FT.EXPLAIN",
    "FT.INFO",
    "FT.SEARCH",
    "FT._LIST",
    "GEODIST",
    "GEOHASH",
    "GEOPOS",
    "GEORADIUSBYMEMBER_RO",
    "GEORADIUS_RO",
    "GEOSEARCH",
    "GET",
    "GETBIT",
    "GETRANGE",
    "HEXISTS",
    "HEXPIRETIME",
    "HGET",
    "HGETALL",
    "HKEYS",
    "HLEN",
    "HMGET",
    "HPEXPIRETIME",
    "HPTTL",
    "HRANDFIELD",
    "HSCAN",
    "HSTRLEN",
    "HTTL",
    "HVALS",
    "INFO",
    "JSON.ARRLEN",
    "JSON.GET",
    "JSON.MGET",
    "JSON.OBJKEYS",
    "JSON.OBJLEN",
    "JSON.RESP",
    "JSON.STRLEN",
    "JSON.TYPE",
    "KEYS",
    "LASTSAVE",
    "LCS",
    "LINDEX",
    "LLEN",
    "LOLWUT",
    "LPOS",
    "LRANGE",
    "MGET",
    "MONITOR",
    "PEXPIRETIME",
    "PFCOUNT",
    "PING",
    "PSUBSCRIBE",
    "PTTL",
    "RANDOMKEY",
    "SCAN",
    "SCARD",
    "SDIFF",
    "SELECT",
    "SINTER",
    "SINTERCARD",
    "SISMEMBER",
    "SMEMBERS",
    "SMISMEMBER",
    "SORT_RO",
    "SRANDMEMBER",
    "SSCAN",
    "SSUBSCRIBE",
    "STRLEN",
    "SUBSCRIBE",
    "SUBSTR",
    "SUNION",
    "TIME",
    "TS.GET",
    "TS.INFO",
    "TS.MRANGE",
    "TS.RANGE",
    "TTL",
    "TYPE",
    "XLEN",
    "XPENDING",
    "XRANGE",
    "XREAD",
    "XREVRANGE",
    "ZCARD",
    "ZCOUNT",
    "ZDIFF",
    "ZINTER",
    "ZINTERCARD",
    "ZLEXCOUNT",
    "ZMSCORE",
    "ZRANDMEMBER",
    "ZRANGE",
    "ZRANGEBYLEX",
    "ZRANGEBYSCORE",
    "ZRANK",
    "ZREVRANGE",
    "ZREVRANGEBYLEX",
    "ZREVRANGEBYSCORE",
    "ZREVRANK",
    "ZSCAN",
    "ZSCORE",
    "ZUNION",
];

/// Erro de leitura do texto, com a posição (em caracteres a partir de 0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub position: usize,
}

/// Divide o texto em comandos.
pub fn split(script: &str) -> Result<Vec<Command>, ParseError> {
    let chars: Vec<char> = script.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        // Começo de linha: espaços, linhas vazias e comentários.
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let comment = chars[i] == '#'
            || (chars[i] == '-' && chars.get(i + 1) == Some(&'-'))
            || (chars[i] == '/' && chars.get(i + 1) == Some(&'/'));
        if comment {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        let start = i;
        let mut args: Vec<Vec<u8>> = Vec::new();
        let mut current: Option<Vec<u8>> = None;
        while i < chars.len() && chars[i] != '\n' {
            let c = chars[i];
            match c {
                '"' | '\'' => {
                    let open = i;
                    let arg = current.get_or_insert_with(Vec::new);
                    i += 1;
                    let mut closed = false;
                    while i < chars.len() {
                        let c2 = chars[i];
                        if c2 == c {
                            closed = true;
                            i += 1;
                            break;
                        }
                        if c2 == '\\' && i + 1 < chars.len() {
                            let next = chars[i + 1];
                            if c == '\'' {
                                if next == '\'' {
                                    arg.push(b'\'');
                                    i += 2;
                                    continue;
                                }
                            } else {
                                i += 2;
                                match next {
                                    'n' => arg.push(b'\n'),
                                    'r' => arg.push(b'\r'),
                                    't' => arg.push(b'\t'),
                                    'b' => arg.push(8),
                                    'a' => arg.push(7),
                                    'x' if i + 1 < chars.len()
                                        && chars[i].is_ascii_hexdigit()
                                        && chars[i + 1].is_ascii_hexdigit() =>
                                    {
                                        let hex: String = chars[i..i + 2].iter().collect();
                                        arg.push(u8::from_str_radix(&hex, 16).unwrap_or(b'?'));
                                        i += 2;
                                    }
                                    other => push_char(arg, other),
                                }
                                continue;
                            }
                        }
                        push_char(arg, c2);
                        i += 1;
                    }
                    if !closed {
                        return Err(ParseError {
                            message: pgstudio_i18n::tr("Unclosed quotation mark in the command.")
                                .to_string(),
                            position: open,
                        });
                    }
                }
                c if c.is_whitespace() => {
                    if let Some(arg) = current.take() {
                        args.push(arg);
                    }
                    i += 1;
                }
                c => {
                    push_char(current.get_or_insert_with(Vec::new), c);
                    i += 1;
                }
            }
        }
        if let Some(arg) = current.take() {
            args.push(arg);
        }
        if !args.is_empty() {
            out.push(Command { args, start });
        }
    }
    Ok(out)
}

fn push_char(arg: &mut Vec<u8>, c: char) {
    let mut buf = [0; 4];
    arg.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// O primeiro comando do texto que pode alterar alguma coisa (o nome, para
/// a confirmação), ou `None` quando o texto só lê. `Err` quando o texto
/// não pôde ser lido.
pub fn first_write(script: &str) -> Result<Option<String>, String> {
    let commands = split(script).map_err(|e| e.message)?;
    Ok(commands
        .iter()
        .find(|c| !c.is_read_only())
        .map(Command::label))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(script: &str) -> Vec<Vec<String>> {
        split(script)
            .unwrap()
            .into_iter()
            .map(|c| {
                c.args
                    .iter()
                    .map(|a| String::from_utf8_lossy(a).into_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn read_only_list_is_sorted() {
        assert!(READ_ONLY.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn splits_lines_quotes_and_comments() {
        assert_eq!(
            args("# começo\nSET nome \"João da Silva\"\n\n  get nome  \n-- fim\n// outro"),
            vec![vec!["SET", "nome", "João da Silva"], vec!["get", "nome"]]
        );
        assert_eq!(
            args(r#"SET k "a\"b\n\x41" 'it\'s' pre"fix""#),
            vec![vec!["SET", "k", "a\"b\nA", "it's", "prefix"]]
        );
        assert_eq!(args("SET k \"linha 1\nlinha 2\"\nGET k").len(), 2);
        assert_eq!(args("SET k \"\""), vec![vec!["SET", "k", ""]]);
        let e = split("GET a\nSET k \"sem fim").unwrap_err();
        assert_eq!(e.position, 12);
    }

    #[test]
    fn commands_know_where_they_start() {
        let c = split("GET a\n  HGETALL açaí\nPING").unwrap();
        assert_eq!(
            c.iter().map(|c| c.start).collect::<Vec<_>>(),
            vec![0, 8, 21]
        );
    }

    #[test]
    fn reads_and_writes() {
        assert_eq!(
            first_write("GET a\nhgetall h\nscan 0\nCONFIG GET *"),
            Ok(None)
        );
        assert_eq!(first_write("GET a\ndel a"), Ok(Some("DEL".into())));
        assert_eq!(first_write("config set a b"), Ok(Some("CONFIG SET".into())));
        assert_eq!(first_write("FLUSHALL"), Ok(Some("FLUSHALL".into())));
        assert_eq!(first_write("MULTI"), Ok(Some("MULTI".into())));
        assert_eq!(first_write("eval \"return 1\" 0"), Ok(Some("EVAL".into())));
        assert_eq!(first_write("POSTPILOT DATABASES"), Ok(None));
        assert!(first_write("GET \"a").is_err());
    }
}
