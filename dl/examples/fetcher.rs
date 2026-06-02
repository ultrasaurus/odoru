use dl::fetch_and_extract;

fn main() -> Result<(), dl::ArticleError> {
    let url: &str = "https://ultrasaurus.com/2019/04/when-reality-is-broken-change-the-rules/";

    match fetch_and_extract(url) {
        Err(dl::ArticleError::ExtractionFailed) => println!("Extraction failed"),
        Err(e) => println!("Error: {}", e),
        Ok(article) => {
            println!("Title: {:?}", article.title);
            println!("Authors: {:?}", article.authors);
            println!("Date: {:?}", article.date);
            println!("\n{}", &article.content);
        }
    }

    Ok(())
}
