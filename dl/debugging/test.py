import trafilatura

url = "https://dougengelbart.org/content/view/148/"
html = trafilatura.fetch_url(url)

# Check raw extraction
text = trafilatura.extract(
    html,
    url=url,
    output_format="txt",
    include_tables=True,
    include_comments=False,
    no_fallback=False,
)

print(repr(text[:3000]))  # repr so you can see \n vs \n\n


