//! Renderer-owned technology glyphs for the top face of a box.
//!
//! Icons are decoration only: they never change shape, role, layout or legend
//! color. Producers name technologies in `technology` (and `subtype`); this
//! module maps those free-form strings to a small set of simplified,
//! monochrome glyphs drawn here in code. Nothing is fetched or supplied.
use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum IconId {
    // Generic, keyed by subtype.
    Relational,
    Document,
    KeyValue,
    Graph,
    Vector,
    Bucket,
    Search,
    Queue,
    Stream,
    Gear,
    Gateway,
    Function,
    Clock,
    Browser,
    Mobile,
    Terminal,
    Shield,
    Key,
    Container,
    Cloud,
    Brackets,
    // Product-evoking marks (simplified, not logos).
    Elephant,
    Leaf,
    Bolt,
    Rabbit,
    Triangle,
    Hexagon,
    Atom,
    Target,
}

const RULES: &[(&[&str], IconId)] = &[
    // Datastores
    (
        &[
            "postgres",
            "pgvector",
            "timescale",
            "supabase",
            "neon",
            "cockroach",
        ],
        IconId::Elephant,
    ),
    (
        &[
            "mysql",
            "mariadb",
            "sqlite",
            "planetscale",
            "turso",
            "oracle",
            "sqlserver",
            "aurora",
            "rds",
        ],
        IconId::Relational,
    ),
    (
        &["mongo", "couchdb", "firestore", "dynamodb", "cosmos"],
        IconId::Leaf,
    ),
    (
        &["redis", "valkey", "keydb", "memcached", "etcd"],
        IconId::Bolt,
    ),
    (&["neo4j", "arango", "neptune", "dgraph"], IconId::Graph),
    (
        &["pinecone", "weaviate", "qdrant", "chroma", "milvus"],
        IconId::Vector,
    ),
    (
        &[
            "elasticsearch",
            "opensearch",
            "algolia",
            "meilisearch",
            "typesense",
            "solr",
        ],
        IconId::Search,
    ),
    (
        &["s3", "gcs", "r2", "minio", "blob", "cloudinary"],
        IconId::Bucket,
    ),
    (
        &[
            "clickhouse",
            "snowflake",
            "bigquery",
            "redshift",
            "duckdb",
            "influx",
            "prometheus",
        ],
        IconId::Stream,
    ),
    // Messaging
    (
        &[
            "kafka",
            "redpanda",
            "pulsar",
            "kinesis",
            "nats",
            "pubsub",
            "eventbridge",
        ],
        IconId::Stream,
    ),
    (
        &[
            "rabbitmq", "sqs", "sns", "bullmq", "sidekiq", "celery", "temporal", "inngest",
        ],
        IconId::Queue,
    ),
    // Auth
    (
        &[
            "clerk", "auth0", "okta", "cognito", "keycloak", "workos", "stytch", "nextauth",
            "authjs", "oauth", "oidc", "saml",
        ],
        IconId::Shield,
    ),
    (&["vault", "1password", "passkey", "webauthn"], IconId::Key),
    // Clients / frameworks
    (
        &[
            "ios",
            "swiftui",
            "android",
            "flutter",
            "expo",
            "reactnative",
        ],
        IconId::Mobile,
    ),
    (
        &["react", "reactjs", "nextjs", "remix", "gatsby"],
        IconId::Atom,
    ),
    (
        &["vue", "nuxt", "svelte", "angular", "astro", "solid"],
        IconId::Triangle,
    ),
    (&["electron", "tauri", "bevy", "unity"], IconId::Browser),
    // Backend
    (
        &[
            "node", "nodejs", "express", "fastify", "nestjs", "deno", "bun", "hono",
        ],
        IconId::Hexagon,
    ),
    (
        &[
            "python", "django", "fastapi", "flask", "rails", "ruby", "spring", "java", "dotnet",
            "laravel", "phoenix", "go", "rust", "golang",
        ],
        IconId::Gear,
    ),
    // API styles
    (
        &["rest", "openapi", "http", "https", "webhook"],
        IconId::Brackets,
    ),
    (
        &[
            "graphql", "apollo", "hasura", "grpc", "protobuf", "trpc", "mcp",
        ],
        IconId::Graph,
    ),
    (&["websocket", "sse", "mqtt"], IconId::Stream),
    // Infra
    (
        &["docker", "kubernetes", "helm", "podman", "ecs"],
        IconId::Container,
    ),
    (
        &[
            "aws",
            "gcp",
            "azure",
            "vercel",
            "netlify",
            "cloudflare",
            "flyio",
            "railway",
            "render",
            "heroku",
            "cloudrun",
        ],
        IconId::Cloud,
    ),
    (
        &["lambda", "workers", "functions", "serverless"],
        IconId::Function,
    ),
    (
        &[
            "nginx",
            "caddy",
            "traefik",
            "envoy",
            "istio",
            "haproxy",
            "cloudfront",
        ],
        IconId::Gateway,
    ),
    (
        &[
            "terraform",
            "pulumi",
            "githubactions",
            "gitlabci",
            "circleci",
            "argo",
        ],
        IconId::Gear,
    ),
    // Observability, payments, AI, comms
    (
        &[
            "datadog",
            "sentry",
            "grafana",
            "opentelemetry",
            "posthog",
            "honeycomb",
            "newrelic",
            "pagerduty",
            "logrocket",
        ],
        IconId::Target,
    ),
    (
        &[
            "stripe",
            "paypal",
            "paddle",
            "lemonsqueezy",
            "shopify",
            "plaid",
        ],
        IconId::Target,
    ),
    (
        &[
            "anthropic",
            "claude",
            "openai",
            "gemini",
            "huggingface",
            "langchain",
            "ollama",
            "vllm",
            "replicate",
            "pytorch",
            "tensorflow",
        ],
        IconId::Atom,
    ),
    (
        &[
            "twilio", "sendgrid", "resend", "postmark", "mailgun", "slack", "discord", "intercom",
            "segment", "smtp",
        ],
        IconId::Rabbit,
    ),
];

const SUBTYPES: &[(&[&str], IconId)] = &[
    (&["relational", "sql", "warehouse"], IconId::Relational),
    (&["document", "nosql"], IconId::Document),
    (&["keyvalue", "kv", "cache"], IconId::KeyValue),
    (&["graph"], IconId::Graph),
    (&["vector"], IconId::Vector),
    (
        &["storage", "object", "blob", "bucket", "file"],
        IconId::Bucket,
    ),
    (&["search", "index"], IconId::Search),
    (&["queue", "broker"], IconId::Queue),
    (&["topic", "event", "stream", "pubsub"], IconId::Stream),
    (
        &[
            "application",
            "app",
            "web_service",
            "service",
            "worker",
            "job",
        ],
        IconId::Gear,
    ),
    (&["gateway", "proxy", "loadbalancer"], IconId::Gateway),
    (&["function", "lambda", "serverless"], IconId::Function),
    (&["scheduler", "cron", "timer"], IconId::Clock),
    (&["web", "browser"], IconId::Browser),
    (&["mobile"], IconId::Mobile),
    (&["cli", "terminal"], IconId::Terminal),
    (
        &["auth", "authentication", "authorization", "security"],
        IconId::Shield,
    ),
    (&["secrets", "config"], IconId::Key),
    (&["container", "vm", "cluster"], IconId::Container),
    (&["cdn", "edge", "region"], IconId::Cloud),
    (&["api", "rest", "rpc", "webhook"], IconId::Brackets),
];

/// Lowercase and drop everything but letters and digits, so "PostgreSQL 16",
/// "postgres" and "Next.js" compare as "postgresql16", "postgres", "nextjs".
fn squash(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The glyph for a box: the first recognised technology wins, then the
/// subtype. Unrecognised input draws nothing, never a guess.
pub(crate) fn icon_for(technology: &[String], subtype: Option<&str>) -> Option<IconId> {
    for tech in technology {
        let key = squash(tech);
        if key.is_empty() {
            continue;
        }
        for (names, icon) in RULES {
            if names.iter().any(|name| matches_name(&key, name)) {
                return Some(*icon);
            }
        }
    }
    let key = squash(subtype?);
    SUBTYPES
        .iter()
        .find(|(names, _)| names.iter().any(|name| key == squash(name)))
        .map(|(_, icon)| *icon)
}

/// Exact, versioned (`go1.22`), or a long name with a suffix (`postgresql`):
/// short names never match as prefixes, so "go" does not claim "google".
fn matches_name(key: &str, name: &str) -> bool {
    key == name || is_versioned(key, name) || (name.len() >= 6 && key.starts_with(name))
}

/// The remainder after `name` is nothing but digits.
fn is_versioned(key: &str, name: &str) -> bool {
    key.strip_prefix(name)
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

// ---- Drawing: glyphs are unions of simple shapes in a unit square. ----

#[derive(Clone, Copy)]
enum Shape {
    /// Centre, half-size, corner radius.
    Rect([f32; 2], [f32; 2], f32),
    Disc([f32; 2], f32),
    Ring([f32; 2], f32, f32),
    /// Capsule between two points with a half-width.
    Line([f32; 2], [f32; 2], f32),
    /// Subtract this shape from what has been drawn so far.
    Cut(&'static Shape),
}

use Shape::{Disc, Line, Rect, Ring};

fn distance(shape: &Shape, p: [f32; 2]) -> f32 {
    match shape {
        Rect(c, h, r) => {
            let q = [
                (p[0] - c[0]).abs() - h[0] + r,
                (p[1] - c[1]).abs() - h[1] + r,
            ];
            let outside = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2)).sqrt();
            outside + q[0].max(q[1]).min(0.0) - r
        }
        Disc(c, r) => ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt() - r,
        Ring(c, r, w) => (((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt() - r).abs() - w,
        Line(a, b, w) => {
            let (ab, ap) = ([b[0] - a[0], b[1] - a[1]], [p[0] - a[0], p[1] - a[1]]);
            let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-6))
                .clamp(0.0, 1.0);
            ((ap[0] - ab[0] * t).powi(2) + (ap[1] - ab[1] * t).powi(2)).sqrt() - w
        }
        Shape::Cut(inner) => distance(inner, p),
    }
}

const W: f32 = 0.06;

fn glyph(icon: IconId) -> Vec<Shape> {
    use IconId::*;
    match icon {
        Relational => vec![
            Rect([0.5, 0.5], [0.4, 0.34], 0.05),
            Shape::Cut(&Line([0.1, 0.5], [0.9, 0.5], 0.025)),
            Shape::Cut(&Line([0.4, 0.16], [0.4, 0.84], 0.025)),
        ],
        Document => vec![
            Line([0.36, 0.2], [0.26, 0.2], W),
            Line([0.26, 0.2], [0.26, 0.8], W),
            Line([0.26, 0.8], [0.36, 0.8], W),
            Line([0.64, 0.2], [0.74, 0.2], W),
            Line([0.74, 0.2], [0.74, 0.8], W),
            Line([0.74, 0.8], [0.64, 0.8], W),
            Disc([0.5, 0.5], 0.07),
        ],
        KeyValue => vec![
            Ring([0.3, 0.5], 0.14, 0.05),
            Line([0.44, 0.5], [0.86, 0.5], 0.05),
            Line([0.74, 0.5], [0.74, 0.66], 0.05),
            Line([0.86, 0.5], [0.86, 0.64], 0.05),
        ],
        Graph => vec![
            Disc([0.25, 0.5], 0.13),
            Disc([0.75, 0.25], 0.13),
            Disc([0.75, 0.75], 0.13),
            Line([0.25, 0.5], [0.75, 0.25], 0.035),
            Line([0.25, 0.5], [0.75, 0.75], 0.035),
        ],
        Vector => vec![
            Disc([0.3, 0.7], 0.1),
            Disc([0.55, 0.45], 0.1),
            Disc([0.78, 0.22], 0.1),
            Line([0.2, 0.8], [0.34, 0.66], 0.035),
        ],
        Bucket => vec![
            Rect([0.5, 0.58], [0.32, 0.3], 0.06),
            Ring([0.5, 0.3], 0.32, 0.045),
            Shape::Cut(&Rect([0.5, 0.22], [0.4, 0.16], 0.0)),
            Rect([0.5, 0.26], [0.32, 0.05], 0.02),
        ],
        Search => vec![
            Ring([0.42, 0.42], 0.26, 0.06),
            Line([0.62, 0.62], [0.86, 0.86], 0.07),
        ],
        Queue => vec![
            Rect([0.2, 0.5], [0.1, 0.22], 0.03),
            Rect([0.5, 0.5], [0.1, 0.22], 0.03),
            Rect([0.8, 0.5], [0.1, 0.22], 0.03),
        ],
        Stream => vec![
            Line([0.1, 0.3], [0.9, 0.3], 0.05),
            Line([0.1, 0.5], [0.9, 0.5], 0.05),
            Line([0.1, 0.7], [0.9, 0.7], 0.05),
            Disc([0.7, 0.3], 0.09),
            Disc([0.35, 0.5], 0.09),
            Disc([0.6, 0.7], 0.09),
        ],
        Gear => vec![
            Disc([0.5, 0.5], 0.3),
            Line([0.5, 0.1], [0.5, 0.9], 0.07),
            Line([0.1, 0.5], [0.9, 0.5], 0.07),
            Line([0.22, 0.22], [0.78, 0.78], 0.07),
            Line([0.22, 0.78], [0.78, 0.22], 0.07),
            Shape::Cut(&Disc([0.5, 0.5], 0.12)),
        ],
        Gateway => vec![
            Line([0.2, 0.85], [0.2, 0.2], 0.06),
            Line([0.8, 0.85], [0.8, 0.2], 0.06),
            Line([0.2, 0.25], [0.8, 0.25], 0.06),
            Line([0.2, 0.55], [0.8, 0.55], 0.04),
            Line([0.2, 0.85], [0.8, 0.85], 0.04),
        ],
        Function => vec![
            Line([0.25, 0.15], [0.7, 0.85], 0.06),
            Line([0.5, 0.5], [0.22, 0.85], 0.06),
            Line([0.2, 0.15], [0.35, 0.15], 0.05),
        ],
        Clock => vec![
            Ring([0.5, 0.5], 0.36, 0.05),
            Line([0.5, 0.5], [0.5, 0.26], 0.045),
            Line([0.5, 0.5], [0.68, 0.58], 0.045),
        ],
        Browser => vec![
            Rect([0.5, 0.5], [0.4, 0.32], 0.05),
            Shape::Cut(&Rect([0.5, 0.58], [0.34, 0.2], 0.02)),
            Disc([0.22, 0.34], 0.04),
            Disc([0.34, 0.34], 0.04),
        ],
        Mobile => vec![
            Rect([0.5, 0.5], [0.22, 0.4], 0.07),
            Shape::Cut(&Rect([0.5, 0.47], [0.16, 0.3], 0.02)),
        ],
        Terminal => vec![
            Line([0.2, 0.3], [0.45, 0.5], 0.06),
            Line([0.45, 0.5], [0.2, 0.7], 0.06),
            Line([0.52, 0.72], [0.8, 0.72], 0.06),
        ],
        Shield => vec![
            Line([0.2, 0.18], [0.8, 0.18], 0.05),
            Line([0.2, 0.18], [0.2, 0.5], 0.05),
            Line([0.8, 0.18], [0.8, 0.5], 0.05),
            Line([0.2, 0.5], [0.5, 0.88], 0.05),
            Line([0.8, 0.5], [0.5, 0.88], 0.05),
            Disc([0.5, 0.45], 0.1),
        ],
        Key => vec![
            Ring([0.3, 0.5], 0.17, 0.06),
            Line([0.46, 0.5], [0.88, 0.5], 0.055),
            Line([0.72, 0.5], [0.72, 0.68], 0.055),
        ],
        Container => vec![
            Rect([0.22, 0.65], [0.1, 0.1], 0.02),
            Rect([0.5, 0.65], [0.1, 0.1], 0.02),
            Rect([0.78, 0.65], [0.1, 0.1], 0.02),
            Rect([0.36, 0.42], [0.1, 0.1], 0.02),
            Rect([0.64, 0.42], [0.1, 0.1], 0.02),
            Line([0.1, 0.84], [0.9, 0.84], 0.04),
        ],
        Cloud => vec![
            Disc([0.34, 0.55], 0.2),
            Disc([0.58, 0.45], 0.24),
            Disc([0.76, 0.6], 0.16),
            Rect([0.5, 0.64], [0.34, 0.12], 0.0),
        ],
        Brackets => vec![
            Line([0.34, 0.25], [0.18, 0.5], 0.06),
            Line([0.18, 0.5], [0.34, 0.75], 0.06),
            Line([0.66, 0.25], [0.82, 0.5], 0.06),
            Line([0.82, 0.5], [0.66, 0.75], 0.06),
            Line([0.56, 0.22], [0.44, 0.78], 0.045),
        ],
        Bolt => vec![
            Line([0.58, 0.1], [0.3, 0.55], 0.07),
            Line([0.3, 0.55], [0.68, 0.5], 0.06),
            Line([0.68, 0.5], [0.42, 0.92], 0.07),
        ],
        // The following are loose, abstract nods to a product's mark.
        Elephant => vec![
            Disc([0.5, 0.36], 0.24),
            Line([0.3, 0.5], [0.3, 0.85], 0.07),
            Line([0.7, 0.5], [0.7, 0.85], 0.07),
            Line([0.5, 0.5], [0.5, 0.88], 0.07),
            Line([0.62, 0.55], [0.86, 0.72], 0.05),
        ],
        Leaf => vec![
            Disc([0.5, 0.42], 0.3),
            Rect([0.5, 0.78], [0.04, 0.16], 0.0),
            Shape::Cut(&Line([0.5, 0.2], [0.5, 0.6], 0.02)),
        ],
        Rabbit => vec![
            Rect([0.5, 0.58], [0.34, 0.24], 0.1),
            Line([0.4, 0.34], [0.34, 0.1], 0.05),
            Line([0.6, 0.34], [0.66, 0.1], 0.05),
        ],
        Triangle => vec![
            Line([0.5, 0.15], [0.12, 0.8], 0.06),
            Line([0.5, 0.15], [0.88, 0.8], 0.06),
            Line([0.12, 0.8], [0.88, 0.8], 0.06),
        ],
        Hexagon => vec![
            Line([0.5, 0.1], [0.84, 0.3], 0.055),
            Line([0.84, 0.3], [0.84, 0.7], 0.055),
            Line([0.84, 0.7], [0.5, 0.9], 0.055),
            Line([0.5, 0.9], [0.16, 0.7], 0.055),
            Line([0.16, 0.7], [0.16, 0.3], 0.055),
            Line([0.16, 0.3], [0.5, 0.1], 0.055),
        ],
        Atom => vec![
            Disc([0.5, 0.5], 0.08),
            Ring([0.5, 0.5], 0.34, 0.03),
            Line([0.16, 0.5], [0.84, 0.5], 0.03),
            Line([0.5, 0.16], [0.5, 0.84], 0.03),
        ],
        IconId::Target => vec![Ring([0.5, 0.5], 0.3, 0.07), Disc([0.5, 0.5], 0.1)],
    }
}

const SIZE: u32 = 96;

/// White-on-transparent coverage mask, antialiased. The caller tints it.
fn rasterize(icon: IconId) -> Image {
    let shapes = glyph(icon);
    let mut data = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    let texel = 1.0 / SIZE as f32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let p = [(x as f32 + 0.5) * texel, (y as f32 + 0.5) * texel];
            let mut coverage = 0.0_f32;
            for shape in &shapes {
                let inside = (0.5 - distance(shape, p) / texel).clamp(0.0, 1.0);
                if matches!(shape, Shape::Cut(_)) {
                    coverage *= 1.0 - inside;
                } else {
                    coverage = coverage.max(inside);
                }
            }
            data.extend([255, 255, 255, (coverage * 255.0).round() as u8]);
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

/// Rasterised glyphs, built on first use and shared by every box.
#[derive(Resource, Default)]
pub(crate) struct IconAtlas(BTreeMap<IconId, Handle<Image>>);

impl IconAtlas {
    pub fn handle(&mut self, icon: IconId, images: &mut Assets<Image>) -> Handle<Image> {
        self.0
            .entry(icon)
            .or_insert_with(|| images.add(rasterize(icon)))
            .clone()
    }
}

/// How a decal sits on a box: a square on the top face, never past its edges.
pub(crate) fn decal_half(half: Vec3) -> f32 {
    (half.x.min(half.z) * 0.55).min(0.6)
}

/// The decal tint: the kind color pulled toward black, so it reads as an
/// inlay and stays below the bloom threshold.
pub(crate) fn tint(base: Color) -> Color {
    let l = base.to_linear();
    Color::linear_rgb(l.red * 0.32, l.green * 0.32, l.blue * 0.32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tech(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn spellings_of_one_product_agree() {
        let a = icon_for(&tech(&["PostgreSQL"]), None);
        assert_eq!(a, Some(IconId::Elephant));
        assert_eq!(icon_for(&tech(&["postgres 16"]), None), a);
        assert_eq!(icon_for(&tech(&["Next.js"]), None), Some(IconId::Atom));
    }

    #[test]
    fn unknown_technology_draws_nothing_and_subtype_is_the_fallback() {
        assert_eq!(icon_for(&tech(&["Acme Internal Thing"]), None), None);
        assert_eq!(icon_for(&[], Some("relational")), Some(IconId::Relational));
        assert_eq!(
            icon_for(&tech(&["Acme"]), Some("queue")),
            Some(IconId::Queue)
        );
        assert_eq!(icon_for(&tech(&[""]), Some("unheard-of")), None);
    }

    #[test]
    fn short_names_do_not_match_longer_words() {
        // "go" must not claim google, "r2" must not claim r2d2 unless versioned digits.
        assert_eq!(icon_for(&tech(&["Google Sheets"]), None), None);
        assert_eq!(icon_for(&tech(&["go1.22"]), None), Some(IconId::Gear));
    }

    #[test]
    fn first_recognised_technology_wins() {
        assert_eq!(
            icon_for(&tech(&["Mystery", "Redis", "PostgreSQL"]), None),
            Some(IconId::Bolt)
        );
    }

    #[test]
    fn every_glyph_draws_something_and_leaves_margin() {
        use IconId::*;
        for icon in [
            Relational,
            Document,
            KeyValue,
            Graph,
            Vector,
            Bucket,
            Search,
            Queue,
            Stream,
            Gear,
            Gateway,
            Function,
            Clock,
            Browser,
            Mobile,
            Terminal,
            Shield,
            Key,
            Container,
            Cloud,
            Brackets,
            Elephant,
            Leaf,
            Bolt,
            Rabbit,
            Triangle,
            Hexagon,
            Atom,
            IconId::Target,
        ] {
            let image = rasterize(icon);
            let data = image.data.as_ref().unwrap();
            let covered = data.chunks(4).filter(|px| px[3] > 128).count();
            assert!(covered > 150, "{icon:?} is nearly empty");
            assert!(
                covered < (SIZE * SIZE) as usize * 3 / 4,
                "{icon:?} is a blob"
            );
        }
    }
}
