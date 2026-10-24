//! A call's symbols to fields (ITU-R M.493). Symbols 0 to 99 carry two decimal digits each; 100 to 127 are
//! format specifiers, categories, telecommands and EOS.

/// Format specifiers.
pub const DISTRESS: u8 = 112;
pub const ALL_SHIPS: u8 = 116;
pub const INDIVIDUAL: u8 = 120;
pub const GROUP: u8 = 114;
pub const AREA: u8 = 102;
/// Categories.
pub const ROUTINE: u8 = 100;
pub const SAFETY: u8 = 108;
pub const URGENCY: u8 = 110;
/// Telecommands that make a call about a distress: acknowledgement and relay.
pub const DISTRESS_ACK: u8 = 110;
pub const DISTRESS_RELAY: u8 = 112;

/// Who a call is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Address {
    /// All ships, or a distress alert (sent to all).
    All,
    /// A station (individual call) or a group, by MMSI.
    Mmsi(u32),
    /// Ships in an area: quadrant (0 NE, 1 NW, 2 SE, 3 SW), latitude and longitude of its corner (degrees), and
    /// its height and width (degrees).
    Area { quadrant: u8, lat: u8, lon: u16, dlat: u8, dlon: u8 },
}

/// A position as DSC sends it: whole minutes, north and east positive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub lat_min: i32,
    pub lon_min: i32,
}

/// What a distress alert (or its acknowledgement or relay) says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Distress {
    /// The vessel in distress.
    pub mmsi: u32,
    /// Nature of distress: 100 fire, 101 flooding, 102 collision, 103 grounding, 104 listing, 105 sinking,
    /// 106 disabled and adrift, 107 undesignated, 108 abandoning ship, 109 piracy, 110 person overboard, 112 EPIRB.
    pub nature: u8,
    /// None when sent as "unknown" (all nines).
    pub position: Option<Position>,
    /// UTC hours and minutes of the position; None when sent as 88 88.
    pub utc: Option<(u8, u8)>,
    /// Subsequent communications: 100 FM voice (F3E), 109 SSB voice (J3E), ...
    pub subsequent: u8,
}

/// A call's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message {
    pub format: u8,
    pub address: Address,
    /// Routine 100, safety 108, urgency 110, distress 112.
    pub category: u8,
    /// The calling station's MMSI.
    pub self_id: u32,
    /// First and second telecommands (type of call, then detail); none in a distress alert.
    pub tc1: Option<u8>,
    pub tc2: Option<u8>,
    /// The distress, for an alert or an acknowledgement or relay of one.
    pub distress: Option<Distress>,
    /// Proposed VHF working channel (frequency field 90 cc cc).
    pub channel: Option<u16>,
    /// Position in a position call (telecommand 121), when present.
    pub position: Option<Position>,
    /// 117 acknowledgement requested, 122 acknowledgement, 127 other.
    pub eos: u8,
}

fn digits(s: u8) -> Option<u32> {
    (s <= 99).then_some(s as u32)
}

/// Ten digits from five symbols; an MMSI is the first nine.
fn mmsi(s: &[u8]) -> Option<u32> {
    let v = s.iter().try_fold(0u64, |v, &x| Some(v * 100 + digits(x)? as u64))?;
    Some((v / 10) as u32)
}

/// Quadrant, DDMM latitude, DDDMM longitude in ten digits; all nines is "unknown".
fn position(s: &[u8]) -> Option<Position> {
    if s.iter().all(|&x| x == 99) {
        return None;
    }
    let mut d = [0u32; 10];
    for (k, &x) in s.iter().enumerate() {
        let v = digits(x)?;
        d[2 * k] = v / 10;
        d[2 * k + 1] = v % 10;
    }
    let lat = ((d[1] * 10 + d[2]) * 60 + d[3] * 10 + d[4]) as i32;
    let lon = ((d[5] * 100 + d[6] * 10 + d[7]) * 60 + d[8] * 10 + d[9]) as i32;
    let (ns, ew) = match d[0] {
        0 => (1, 1),
        1 => (1, -1),
        2 => (-1, 1),
        3 => (-1, -1),
        _ => return None,
    };
    Some(Position { lat_min: ns * lat, lon_min: ew * lon })
}

fn utc(s: &[u8]) -> Option<(u8, u8)> {
    (s[0] <= 23 && s[1] <= 59).then_some((s[0], s[1]))
}

/// Nature, position, time, subsequent communications: from `s` on.
fn distress(mmsi_: u32, s: &[u8]) -> Option<Distress> {
    let s = s.get(..9)?;
    Some(Distress { mmsi: mmsi_, nature: s[0], position: position(&s[1..6]), utc: utc(&s[6..8]), subsequent: s[8] })
}

impl Message {
    /// Fields from a call's symbols (both format specifiers to the EOS). None if the call is too short for its
    /// format or a field is out of range.
    pub fn parse(s: &[u8]) -> Option<Self> {
        let (&format, &eos) = (s.first()?, s.last()?);
        let mut m = Message {
            format,
            address: Address::All,
            category: 0,
            self_id: 0,
            tc1: None,
            tc2: None,
            distress: None,
            channel: None,
            position: None,
            eos,
        };
        let body = s.get(2..s.len() - 1)?; // after the two format specifiers, before the EOS
        let rest = match format {
            DISTRESS => {
                m.category = DISTRESS;
                m.self_id = mmsi(body.get(..5)?)?;
                m.distress = Some(distress(m.self_id, body.get(5..)?)?);
                return Some(m);
            }
            ALL_SHIPS => body,
            INDIVIDUAL | GROUP => {
                m.address = Address::Mmsi(mmsi(body.get(..5)?)?);
                &body[5..]
            }
            AREA => {
                let a = body.get(..5)?;
                let d = |k: usize| digits(a[k]);
                let (q, lat, lon, dlat, dlon) = (d(0)? / 10, d(0)? % 10 * 10 + d(1)? / 10, d(1)? % 10 * 100 + d(2)?, d(3)?, d(4)?);
                m.address = Address::Area { quadrant: q as u8, lat: lat as u8, lon: lon as u16, dlat: dlat as u8, dlon: dlon as u8 };
                &body[5..]
            }
            _ => return None,
        };
        m.category = *rest.first()?;
        m.self_id = mmsi(rest.get(1..6)?)?;
        let tc1 = *rest.get(6)?;
        m.tc1 = Some(tc1);
        if m.category == DISTRESS && (tc1 == DISTRESS_ACK || tc1 == DISTRESS_RELAY) {
            // About someone else's distress: their MMSI, then the alert's fields.
            let r = rest.get(7..)?;
            m.distress = Some(distress(mmsi(r.get(..5)?)?, r.get(5..)?)?);
            return Some(m);
        }
        m.tc2 = Some(*rest.get(7)?);
        let data = rest.get(8..).unwrap_or(&[]);
        if tc1 == 121 {
            m.position = data.get(..5).and_then(position);
        } else if data.len() >= 3 && data[0] == 90 {
            m.channel = Some(digits(data[1])? as u16 * 100 + digits(data[2])? as u16);
        }
        Some(m)
    }
}
