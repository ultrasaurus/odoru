use clap::{Parser, ValueEnum};
use dl::{fetch_and_extract, ParsedArticle, OutputFormat};

#[derive(Parser)]
#[command(name = "dl", about = "Download a web article as markdown or plain text")]
struct Cli {
    /// URL to fetch and extract
    url: String,

    /// Output format
    #[arg(long, value_enum, default_value_t = Format::Markdown)]
    format: Format,

    /// Include YAML frontmatter with article metadata
    #[arg(long)]
    frontmatter: bool,
}

#[derive(ValueEnum, Clone)]
enum Format {
    Markdown,
    Text,
}

fn render_frontmatter(article: &ParsedArticle) -> String {
    let mut fm = String::from("---\n");

    fm.push_str(&format!(
        "title: \"{}\"\n",
        article.title.as_deref().unwrap_or("~").replace('"', "\\\"")
    ));
    fm.push_str(&format!("url: {}\n", article.url));

    fm.push_str("authors:\n");
    if article.authors.is_empty() {
        fm.push_str("  - ~\n");
    } else {
        for author in &article.authors {
            fm.push_str(&format!("  - {}\n", author));
        }
    }

    fm.push_str(&format!(
        "date: {}\n",
        article.date.as_deref().unwrap_or("~")
    ));
    fm.push_str(&format!(
        "description: \"{}\"\n",
        article.description.as_deref().unwrap_or("~").replace('"', "\\\"")
    ));

    fm.push_str("---\n\n");
    fm
}

fn double_space_paragraphs(text: &str) -> String {
    text.lines()
        .collect::<Vec<_>>()
        .windows(2)
        .fold(String::new(), |mut acc, w| {
            acc.push_str(w[0]);
            acc.push('\n');
            if !w[0].is_empty() && !w[1].is_empty() {
                acc.push('\n');
            }
            acc
        })
}

fn main() -> Result<(), dl::ArticleError> {
    let cli = Cli::parse();

    let format = match cli.format {
        Format::Markdown => OutputFormat::Markdown,
        Format::Text => OutputFormat::Text,
    };

    match fetch_and_extract(&cli.url, format) {
        Err(dl::ArticleError::ExtractionFailed) => eprintln!("Extraction failed"),
        Err(e) => eprintln!("Error: {}", e),
        Ok(article) => {
            if cli.frontmatter {
                print!("{}", render_frontmatter(&article));
            }
            let content = match cli.format {
                Format::Text => double_space_paragraphs(&article.content),
                Format::Markdown => article.content.clone(),
            };
            print!("{}", content);
        }
    }

    Ok(())
}