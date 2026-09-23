//! Which environment variables source code reads, found by the accessor each
//! language spells out. Pure over text: `check` feeds it the repository's files.

/// The accessors, each followed by the name: bare (`.NAME`) or quoted (`("NAME")`).
const ACCESSORS: &[(&str, Form)] = &[
    ("process.env.", Form::Dotted),
    ("process.env[", Form::Quoted),
    ("import.meta.env.", Form::Dotted),
    ("import.meta.env[", Form::Quoted),
    ("Deno.env.get(", Form::Quoted),
    ("Netlify.env.get(", Form::Quoted),
    ("os.environ[", Form::Quoted),
    ("os.environ.get(", Form::Quoted),
    ("os.getenv(", Form::Quoted),
    ("env::var(", Form::Quoted),
    ("env::var_os(", Form::Quoted),
    ("os.Getenv(", Form::Quoted),
    ("os.LookupEnv(", Form::Quoted),
    ("System.getenv(", Form::Quoted),
    ("Environment.GetEnvironmentVariable(", Form::Quoted),
    ("ENV[", Form::Quoted),
    ("ENV.fetch(", Form::Quoted),
    ("$_ENV[", Form::Quoted),
    ("getenv(", Form::Quoted),
    // Prisma's schema.prisma: url = env("DATABASE_URL")
    ("env(", Form::Quoted),
];

#[derive(Clone, Copy)]
enum Form {
    Dotted,
    Quoted,
}

/// Names read, with the 1-based line of each first read.
pub fn reads(text: &str) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        for (accessor, form) in ACCESSORS {
            let mut from = 0;
            while let Some(at) = line[from..].find(accessor) {
                let start = from + at;
                from = start + accessor.len();
                // `getenv(` inside `os.getenv(`, `env(` inside `getenv(`: the
                // longer accessor already counted it.
                // `std::env::var(` is `env::var(` behind a path.
                if preceded_by_word(line, start)
                    && !(accessor.starts_with("env::") && line[..start].ends_with("::"))
                {
                    continue;
                }
                let rest = &line[from..];
                let name = match form {
                    Form::Dotted => ident(rest),
                    Form::Quoted => quoted(rest),
                };
                if let Some(name) = name
                    && !out.iter().any(|(n, _)| *n == name)
                {
                    out.push((name, index + 1));
                }
            }
        }
    }
    out
}

fn preceded_by_word(line: &str, at: usize) -> bool {
    line[..at]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == ':')
}

fn ident(text: &str) -> Option<String> {
    let name: String = text
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    valid(&name).then_some(name)
}

fn quoted(text: &str) -> Option<String> {
    let text = text.trim_start();
    let quote = text
        .chars()
        .next()
        .filter(|c| matches!(c, '"' | '\'' | '`'))?;
    let inner = &text[1..];
    let end = inner.find(quote)?;
    let name = &inner[..end];
    valid(name).then(|| name.to_string())
}

fn valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Variables every platform or tool sets, which no schema is expected to declare.
pub fn is_ambient(name: &str) -> bool {
    const NAMES: &[&str] = &[
        "NODE_ENV",
        "PATH",
        "HOME",
        "PWD",
        "USER",
        "SHELL",
        "TERM",
        "TZ",
        "LANG",
        "HOSTNAME",
        "TMPDIR",
        "TEMP",
        "TMP",
        "CI",
        "DEBUG",
        "NO_COLOR",
        "FORCE_COLOR",
        "NEXT_RUNTIME",
        "NEXT_PHASE",
        "MODE",
        "DEV",
        "PROD",
        "SSR",
        "BASE_URL",
        "APPDATA",
        "LOCALAPPDATA",
        "USERPROFILE",
        "PENV_ENV",
    ];
    const PREFIXES: &[&str] = &[
        "npm_",
        "VERCEL",
        "NETLIFY",
        "GITHUB_",
        "RUNNER_",
        "CF_",
        "RENDER_",
        "RAILWAY_",
        "FLY_",
        "AWS_LAMBDA_",
        "LAMBDA_",
        "K_",
        "PENV_",
        "XDG_",
        "LC_",
    ];
    NAMES.contains(&name) || PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Whether `name` appears as a whole word in `text`: how a generated accessor
/// (`env.STRIPE_SECRET_KEY`) or a framework's own config names a key.
pub fn mentions(text: &str, name: &str) -> bool {
    let mut from = 0;
    while let Some(at) = text[from..].find(name) {
        let start = from + at;
        let end = start + name.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        if !word(before) && !word(after) {
            return true;
        }
        from = end;
    }
    false
}

/// The files worth reading for accessors.
pub fn is_source(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default(),
        "js" | "jsx"
            | "mjs"
            | "cjs"
            | "ts"
            | "tsx"
            | "mts"
            | "cts"
            | "vue"
            | "svelte"
            | "astro"
            | "py"
            | "rb"
            | "go"
            | "rs"
            | "php"
            | "java"
            | "kt"
            | "kts"
            | "cs"
            | "swift"
            | "ex"
            | "exs"
            | "prisma"
            | "sh"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<String> {
        let mut out: Vec<String> = reads(text).into_iter().map(|(n, _)| n).collect();
        out.sort();
        out
    }

    #[test]
    fn each_language_spelling_is_found() {
        let text = r#"
const a = process.env.STRIPE_KEY; const b = process.env["DB_URL"];
const c = import.meta.env.VITE_API; Deno.env.get('DENO_KEY');
os.environ["PY_A"]; os.environ.get("PY_B"); os.getenv('PY_C')
std::env::var("RS_A"); os.Getenv("GO_A"); System.getenv("JAVA_A");
Environment.GetEnvironmentVariable("CS_A"); ENV["RB_A"]; ENV.fetch("RB_B"); getenv('PHP_A'); $_ENV['PHP_B'];
  url = env("PRISMA_URL")
"#;
        let mut want = vec![
            "STRIPE_KEY",
            "DB_URL",
            "VITE_API",
            "DENO_KEY",
            "PY_A",
            "PY_B",
            "PY_C",
            "RS_A",
            "GO_A",
            "JAVA_A",
            "CS_A",
            "RB_A",
            "RB_B",
            "PHP_A",
            "PHP_B",
            "PRISMA_URL",
        ];
        want.sort();
        assert_eq!(names(text), want);
    }

    #[test]
    fn a_word_that_merely_contains_an_accessor_is_not_one() {
        assert!(names("myenv(\"X\"); dotenv(\"Y\"); obj.getenv(\"Z\")").is_empty());
        assert!(
            names("process.env[name]; process.env.").is_empty(),
            "a computed or cut-off name is no name"
        );
    }

    #[test]
    fn mentions_are_whole_words() {
        assert!(mentions("env.STRIPE_KEY)", "STRIPE_KEY"));
        assert!(!mentions("STRIPE_KEY_2", "STRIPE_KEY"));
        assert!(!mentions("MY_STRIPE_KEY", "STRIPE_KEY"));
    }
}
