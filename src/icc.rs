//! Gerador de perfis ICC v2 com tabela B2A (lut16) que aplica "digital vibrance".
//!
//! O KWin (Plasma 6.1+) entende perfis ICC baseados em LUT. Um perfil sintético
//! pode descrever qualquer transformação de saída, inclusive a matriz de
//! saturação que os drivers do Windows aplicam no sinal já codificado em gama.
//!
//! O algoritmo e as constantes seguem o projeto MIT `Celiz/plasma-vibrance`
//! (https://github.com/Celiz/plasma-vibrance), reescrito aqui em Rust puro
//! para não depender de liblcms2. Estágios do tag B2A:
//!
//! 1. matriz 3x3: XYZ D50 (PCS, como o KWin codifica) -> sRGB linear
//! 2. curvas 1D de entrada: codificação gama 2.2 (negativos recortados em 0)
//! 3. CLUT 17^3: `out = Y' + s * (c - Y')` guardado com offset para não recortar
//! 4. curvas 1D de saída: desfaz o offset e recorta em [0, 1]
//!
//! A CLUT contém só uma função linear, então a interpolação trilinear do KWin
//! reproduz a transformação ideal sem erro apreciável.

const GAMMA: f64 = 2.2;
/// Luma BT.709.
const W: [f64; 3] = [0.2126, 0.7152, 0.0722];
/// sRGB linear -> XYZ (D65).
const M_SRGB: [[f64; 3]; 3] = [
    [0.4124564, 0.3575761, 0.1804375],
    [0.2126729, 0.7151522, 0.0721750],
    [0.0193339, 0.1191920, 0.9503041],
];
/// Adaptação cromática Bradford D50 -> D65.
const BRADFORD_D50_TO_D65: [[f64; 3]; 3] = [
    [0.9555766, -0.0230393, 0.0631636],
    [-0.0282895, 1.0099416, 0.0210077],
    [0.0122982, -0.0204830, 1.3299098],
];
/// O KWin escala o XYZ assim antes de entrar no tag B2A.
const KWIN_XYZ_MULT: f64 = 65536.0 / (2.0 * 65535.0);
/// A CLUT guarda o intervalo [-2, 3]: sem recorte até saturação 3x.
const CLUT_OFF: f64 = 2.0;
const CLUT_SPAN: f64 = 5.0;
const CLUT_GRID: usize = 17;
const TABLE_ENTRIES: usize = 4096;

/// Saturação máxima suportada sem recorte na CLUT.
pub const MAX_FACTOR: f64 = 3.0;

fn matmul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

fn inv(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    [
        [(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det],
        [(f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det],
        [(d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det],
    ]
}

/// Conversão para s15Fixed16, igual ao lcms (`floor(v * 65536 + 0.5)`).
fn s15f16(v: f64) -> i32 {
    (v * 65536.0 + 0.5).floor() as i32
}

/// Arredondamento para 16 bits, igual ao lcms (`_cmsQuickSaturateWord`).
fn q16(v: f64) -> u16 {
    let v = v * 65535.0 + 0.5;
    if v.is_nan() || v <= 0.0 {
        0
    } else if v >= 65535.0 {
        65535
    } else {
        v.floor() as u16
    }
}

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn sig(&mut self, s: &[u8; 4]) {
        self.0.extend_from_slice(s);
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    fn zeros(&mut self, n: usize) {
        self.0.resize(self.0.len() + n, 0);
    }
}

fn xyz_tag(x: i32, y: i32, z: i32) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.sig(b"XYZ ");
    w.u32(0);
    w.i32(x);
    w.i32(y);
    w.i32(z);
    w.0
}

/// `textDescriptionType` (ICC v2), com versão Unicode como o lcms escreve.
fn desc_tag(text: &str) -> Vec<u8> {
    let ascii: Vec<u8> = text
        .chars()
        .map(|c| if c.is_ascii() && c != '\0' { c as u8 } else { b'?' })
        .collect();
    let mut w = Writer(Vec::new());
    w.sig(b"desc");
    w.u32(0);
    w.u32(ascii.len() as u32 + 1);
    w.bytes(&ascii);
    w.u8(0);
    w.u32(0); // idioma Unicode
    w.u32(ascii.len() as u32 + 1);
    for &c in &ascii {
        w.u16(c as u16);
    }
    w.u16(0);
    w.u16(0); // ScriptCode code
    w.u8(0); // ScriptCode count
    w.zeros(67); // descrição Macintosh
    let pad = w.0.len().div_ceil(4) * 4 - w.0.len(); // o lcms grava o tamanho já alinhado
    w.zeros(pad);
    w.0
}

fn text_tag(text: &str) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.sig(b"text");
    w.u32(0);
    w.bytes(text.as_bytes());
    w.u8(0);
    w.0
}

/// Colorantes sRGB adaptados a D50 (s15Fixed16), como o lcms grava.
const R_XYZ: (i32, i32, i32) = (0x6fa0, 0x38f5, 0x0390);
const G_XYZ: (i32, i32, i32) = (0x6297, 0xb787, 0x18d9);
const B_XYZ: (i32, i32, i32) = (0x249f, 0x0f84, 0xb6c3);
const D50: (i32, i32, i32) = (0xf6d6, 0x1_0000, 0xd32d);
/// Matriz de adaptação cromática D65 -> D50 (Bradford), s15Fixed16.
const CHAD: [i32; 9] = [
    0x00010c42, 0x000005de, -0x0cdb, 0x00000793, 0x0000fd90, -0x045f, -0x025e, 0x000003dc,
    0x0000c06e,
];

fn chad_tag() -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.sig(b"sf32");
    w.u32(0);
    for v in CHAD {
        w.i32(v);
    }
    w.0
}

fn curv_gamma_tag() -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.sig(b"curv");
    w.u32(0);
    w.u32(1);
    w.u16((GAMMA * 256.0 + 0.5) as u16); // u8Fixed8
    w.0
}

fn chrm_tag() -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.sig(b"chrm");
    w.u32(0);
    w.u16(3);
    w.u16(0);
    for (x, y) in [(0.64, 0.33), (0.30, 0.60), (0.15, 0.06)] {
        w.u32(s15f16(x) as u32);
        w.u32(s15f16(y) as u32);
    }
    w.0
}

/// Tag `B2A` do tipo `lut16Type` (`mft2`) com a transformação de saturação.
fn b2a_tag(s: f64) -> Vec<u8> {
    let pcs_to_srgb = matmul(&inv(&M_SRGB), &BRADFORD_D50_TO_D65);
    let n = CLUT_GRID;

    let mut w = Writer(Vec::with_capacity(80_000));
    w.sig(b"mft2");
    w.u32(0);
    w.u8(3); // canais de entrada
    w.u8(3); // canais de saída
    w.u8(n as u8);
    w.u8(0);
    for row in &pcs_to_srgb {
        for v in row {
            w.i32(s15f16(v / KWIN_XYZ_MULT));
        }
    }
    w.u16(TABLE_ENTRIES as u16);
    w.u16(TABLE_ENTRIES as u16);

    // curvas de entrada: codificação gama
    let enc: Vec<u16> = (0..TABLE_ENTRIES)
        .map(|i| q16((i as f64 / (TABLE_ENTRIES - 1) as f64).powf(1.0 / GAMMA)))
        .collect();
    for _ in 0..3 {
        for &v in &enc {
            w.u16(v);
        }
    }

    // CLUT: saturação em espaço gama; o primeiro canal varia mais devagar
    for i in 0..n {
        for j in 0..n {
            for l in 0..n {
                let r = i as f64 / (n - 1) as f64;
                let g = j as f64 / (n - 1) as f64;
                let b = l as f64 / (n - 1) as f64;
                let y = W[0] * r + W[1] * g + W[2] * b;
                for c in [r, g, b] {
                    let v = y + s * (c - y);
                    w.u16(q16((v + CLUT_OFF) / CLUT_SPAN));
                }
            }
        }
    }

    // curvas de saída: desfazem o offset e recortam em [0, 1]
    let out: Vec<u16> = (0..TABLE_ENTRIES)
        .map(|i| {
            let x = i as f64 / (TABLE_ENTRIES - 1) as f64;
            q16((x * CLUT_SPAN - CLUT_OFF).clamp(0.0, 1.0))
        })
        .collect();
    for _ in 0..3 {
        for &v in &out {
            w.u16(v);
        }
    }
    w.0
}

/// Data/hora UTC (ano, mês, dia, hora, min, seg) a partir do relógio do sistema.
fn utc_now() -> [u16; 6] {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    [
        y as u16,
        m as u16,
        d as u16,
        (rem / 3600) as u16,
        (rem % 3600 / 60) as u16,
        (rem % 60) as u16,
    ]
}

/// Monta o perfil ICC completo para o fator de saturação `s` (1.0 = neutro).
pub fn build_profile(description: &str, s: f64) -> Vec<u8> {
    let s = s.clamp(0.0, MAX_FACTOR);
    let b2a = b2a_tag(s);

    // (assinatura, índice do blob); o lcms grava B2A0 e B2A1 como cópias separadas
    let mut tags: Vec<([u8; 4], usize)> = Vec::new();
    let mut blobs: Vec<Vec<u8>> = vec![
        desc_tag(description),
        text_tag("No copyright, use freely"),
        xyz_tag(D50.0, D50.1, D50.2),
        chad_tag(),
        xyz_tag(R_XYZ.0, R_XYZ.1, R_XYZ.2),
        xyz_tag(B_XYZ.0, B_XYZ.1, B_XYZ.2),
        xyz_tag(G_XYZ.0, G_XYZ.1, G_XYZ.2),
        curv_gamma_tag(),
        chrm_tag(),
        b2a.clone(),
        b2a,
    ];
    let sigs: [&[u8; 4]; 13] = [
        b"desc", b"cprt", b"wtpt", b"chad", b"rXYZ", b"bXYZ", b"gXYZ", b"rTRC", b"gTRC", b"bTRC",
        b"chrm", b"B2A0", b"B2A1",
    ];
    // índice do blob de cada tag
    let blob_of = [0, 1, 2, 3, 4, 5, 6, 7, 7, 7, 8, 9, 10];
    for (sig, &bi) in sigs.iter().zip(blob_of.iter()) {
        tags.push((**sig, bi));
    }

    let table_len = 4 + tags.len() * 12;
    let mut offsets = vec![0usize; blobs.len()];
    let mut pos = 128 + table_len;
    for (i, b) in blobs.iter_mut().enumerate() {
        offsets[i] = pos;
        let len = b.len();
        let padded = len.div_ceil(4) * 4;
        pos += padded;
    }
    let total = pos;

    let mut w = Writer(Vec::with_capacity(total));
    // cabeçalho (128 bytes)
    w.u32(total as u32);
    w.sig(b"lcms");
    w.u32(0x0210_0000); // v2.1.0
    w.sig(b"mntr");
    w.sig(b"RGB ");
    w.sig(b"XYZ ");
    for v in utc_now() {
        w.u16(v);
    }
    w.sig(b"acsp");
    w.u32(0); // plataforma
    w.u32(0); // flags
    w.u32(0); // fabricante
    w.u32(0); // modelo
    w.zeros(8); // atributos
    w.u32(0); // intenção perceptual
    w.i32(D50.0);
    w.i32(D50.1);
    w.i32(D50.2);
    w.sig(b"lcms"); // criador
    w.zeros(16); // ID do perfil
    w.zeros(28);
    debug_assert_eq!(w.0.len(), 128);

    // tabela de tags
    w.u32(tags.len() as u32);
    for (sig, bi) in &tags {
        w.sig(sig);
        w.u32(offsets[*bi] as u32);
        w.u32(blobs[*bi].len() as u32);
    }
    for b in &blobs {
        w.bytes(b);
        let pad = b.len().div_ceil(4) * 4 - b.len();
        w.zeros(pad);
    }
    debug_assert_eq!(w.0.len(), total);
    w.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_layout_is_consistent() {
        let p = build_profile("teste", 1.5);
        let size = u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize;
        assert_eq!(size, p.len());
        assert_eq!(&p[36..40], b"acsp");
        let count = u32::from_be_bytes(p[128..132].try_into().unwrap()) as usize;
        assert_eq!(count, 13);
        for t in 0..count {
            let e = 132 + t * 12;
            let off = u32::from_be_bytes(p[e + 4..e + 8].try_into().unwrap()) as usize;
            let len = u32::from_be_bytes(p[e + 8..e + 12].try_into().unwrap()) as usize;
            assert_eq!(off % 4, 0);
            assert!(off + len <= p.len());
        }
    }

    #[test]
    fn neutral_factor_gives_identity_clut_on_grays() {
        // com s = 1, a CLUT de um cinza (r=g=b) deve devolver o próprio cinza
        let b2a = b2a_tag(1.0);
        let n = CLUT_GRID;
        let clut_start = 48 + 4 + 3 * TABLE_ENTRIES * 2;
        for k in 0..n {
            let idx = ((k * n + k) * n + k) * 3;
            let o = clut_start + idx * 2;
            let v = u16::from_be_bytes([b2a[o], b2a[o + 1]]) as f64 / 65535.0;
            let decoded = v * CLUT_SPAN - CLUT_OFF;
            assert!((decoded - k as f64 / (n - 1) as f64).abs() < 1e-4);
        }
    }
}
