//! Valores do SQL Server como o texto que o SSMS mostra na grade.

use tiberius::ColumnData;
use tiberius::ColumnType;

/// O valor da célula como texto; `None` é NULL.
pub fn to_text(data: &ColumnData<'_>, ty: ColumnType) -> Option<String> {
    Some(match data {
        ColumnData::U8(v) => v.as_ref()?.to_string(),
        ColumnData::I16(v) => v.as_ref()?.to_string(),
        ColumnData::I32(v) => v.as_ref()?.to_string(),
        ColumnData::I64(v) => v.as_ref()?.to_string(),
        ColumnData::F32(v) => float(f64::from(*v.as_ref()?)),
        ColumnData::F64(v) => {
            let v = *v.as_ref()?;
            match ty {
                // money e smallmoney: quatro casas, como no SSMS.
                ColumnType::Money | ColumnType::Money4 => format!("{v:.4}"),
                _ => float(v),
            }
        }
        ColumnData::Bit(v) => if *v.as_ref()? { "1" } else { "0" }.to_string(),
        ColumnData::String(v) => v.as_ref()?.to_string(),
        ColumnData::Guid(v) => v.as_ref()?.to_string().to_uppercase(),
        ColumnData::Binary(v) => {
            let bytes = v.as_ref()?;
            let mut s = String::with_capacity(2 + bytes.len() * 2);
            s.push_str("0x");
            for b in bytes.iter() {
                s.push_str(&format!("{b:02X}"));
            }
            s
        }
        ColumnData::Numeric(v) => v.as_ref()?.to_string(),
        ColumnData::Xml(v) => v.as_ref()?.to_string(),
        ColumnData::DateTime(v) => {
            let v = v.as_ref()?;
            // Dias desde 1900-01-01 e frações de 1/300 de segundo.
            let ms = (u64::from(v.seconds_fragments()) * 1000 + 150) / 300;
            format!(
                "{} {}",
                date(i64::from(v.days()) + DAYS_0001_TO_1900),
                clock(ms * 10_000, 3)
            )
        }
        ColumnData::SmallDateTime(v) => {
            let v = v.as_ref()?;
            let minutes = u64::from(v.seconds_fragments());
            format!(
                "{} {}",
                date(i64::from(v.days()) + DAYS_0001_TO_1900),
                clock(minutes * 60 * 10_000_000, 0)
            )
        }
        ColumnData::Time(v) => {
            let v = v.as_ref()?;
            clock(ticks(v.increments(), v.scale()), 7)
        }
        ColumnData::Date(v) => date(i64::from(v.as_ref()?.days())),
        ColumnData::DateTime2(v) => {
            let v = v.as_ref()?;
            let t = v.time();
            format!(
                "{} {}",
                date(i64::from(v.date().days())),
                clock(ticks(t.increments(), t.scale()), t.scale())
            )
        }
        ColumnData::DateTimeOffset(v) => {
            let v = v.as_ref()?;
            let dt = v.datetime2();
            let t = dt.time();
            // O valor vem em UTC; o SSMS mostra a hora local do valor.
            let offset = i64::from(v.offset());
            let total = i64::from(dt.date().days()) * DAY_TICKS
                + ticks(t.increments(), t.scale()) as i64
                + offset * 60 * 10_000_000;
            let days = total.div_euclid(DAY_TICKS);
            let rest = total.rem_euclid(DAY_TICKS) as u64;
            let sign = if offset < 0 { '-' } else { '+' };
            format!(
                "{} {} {sign}{:02}:{:02}",
                date(days),
                clock(rest, t.scale()),
                offset.abs() / 60,
                offset.abs() % 60
            )
        }
    })
}

/// Nome do tipo para o cabeçalho e para o Claude.
pub fn type_name(ty: ColumnType) -> &'static str {
    match ty {
        ColumnType::Null => "null",
        ColumnType::Bit | ColumnType::Bitn => "bit",
        ColumnType::Int1 => "tinyint",
        ColumnType::Int2 => "smallint",
        ColumnType::Int4 | ColumnType::Intn => "int",
        ColumnType::Int8 => "bigint",
        ColumnType::Datetime4 => "smalldatetime",
        ColumnType::Float4 => "real",
        ColumnType::Float8 | ColumnType::Floatn => "float",
        ColumnType::Money | ColumnType::Money4 => "money",
        ColumnType::Datetime | ColumnType::Datetimen => "datetime",
        ColumnType::Guid => "uniqueidentifier",
        ColumnType::Decimaln | ColumnType::Numericn => "decimal",
        ColumnType::Daten => "date",
        ColumnType::Timen => "time",
        ColumnType::Datetime2 => "datetime2",
        ColumnType::DatetimeOffsetn => "datetimeoffset",
        ColumnType::BigVarBin | ColumnType::BigBinary | ColumnType::Image => "varbinary",
        ColumnType::BigVarChar | ColumnType::BigChar | ColumnType::Text => "varchar",
        ColumnType::NVarchar | ColumnType::NChar | ColumnType::NText => "nvarchar",
        ColumnType::Xml => "xml",
        ColumnType::Udt => "udt",
        ColumnType::SSVariant => "sql_variant",
    }
}

const DAY_TICKS: i64 = 86_400 * 10_000_000;

/// Dias de 0001-01-01 até 1900-01-01.
const DAYS_0001_TO_1900: i64 = 693_595;

fn float(v: f64) -> String {
    if v.is_finite() && v == v.trunc() && v.abs() < 1e15 {
        format!("{v:.0}")
    } else {
        v.to_string()
    }
}

/// Incrementos de 10^-scale segundo em unidades de 100 ns.
fn ticks(increments: u64, scale: u8) -> u64 {
    let scale = u32::from(scale.min(7));
    increments * 10u64.pow(7 - scale)
}

/// `hh:mm:ss` mais `digits` casas de fração; `ticks` em 100 ns.
fn clock(ticks: u64, digits: u8) -> String {
    let secs = ticks / 10_000_000;
    let mut s = format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
    if digits > 0 {
        let frac = ticks % 10_000_000;
        let digits = usize::from(digits.min(7));
        s.push('.');
        s.push_str(&format!("{frac:07}")[..digits]);
    }
    s
}

/// `aaaa-mm-dd` de dias desde 0001-01-01 (calendário gregoriano).
fn date(days: i64) -> String {
    // Algoritmo de Howard Hinnant, a partir de 1970-01-01.
    let z = days - 719_162 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_from_day_counts() {
        assert_eq!(date(0), "0001-01-01");
        assert_eq!(date(DAYS_0001_TO_1900), "1900-01-01");
        assert_eq!(date(719_162), "1970-01-01");
        assert_eq!(date(739_569), "2025-11-15");
    }

    #[test]
    fn clocks_with_fractions() {
        assert_eq!(clock(0, 0), "00:00:00");
        assert_eq!(
            clock(ticks(1_234_567, 7) + 3_600 * 10_000_000, 7),
            "01:00:00.1234567"
        );
        assert_eq!(clock(ticks(5, 1), 1), "00:00:00.5");
    }

    #[test]
    fn floats_without_noise() {
        assert_eq!(float(3.0), "3");
        assert_eq!(float(2.5), "2.5");
    }
}
