use dl::{fetch_and_extract, OutputFormat};

fn main() -> Result<(), dl::ArticleError> {
    // let url: &str = "https://ultrasaurus.com/2019/04/when-reality-is-broken-change-the-rules/";
    let url: &str = "https://dougengelbart.org/content/view/148/";

    match fetch_and_extract(url, OutputFormat::Markdown) {
        Err(dl::ArticleError::ExtractionFailed) => println!("Extraction failed"),
        Err(e) => println!("Error: {}", e),
        Ok(article) => {
            println!("Title: {:?}", article.title);
            println!("Authors: {:?}", article.authors);
            println!("Date: {:?}", article.date);
            // println!("\n{}", &article.content[..2000.min(article.content.len())]);
            println!("\n{}", &article.content);
        }
    }

    Ok(())
}
