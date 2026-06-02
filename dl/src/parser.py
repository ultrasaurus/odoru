import trafilatura


def extract_metadata(html: str, url: str) -> dict:
    """Extract only metadata via trafilatura. Used when content extraction is handled elsewhere."""
    meta = trafilatura.bare_extraction(
        html,
        url=url,
        include_tables=False,
        include_comments=False,
        no_fallback=False,
        with_metadata=True,
        as_dict=True,
    ) or {}

    return {
        "title": meta.get("title"),
        "authors": meta.get("author"),
        "date": meta.get("date"),
        "description": meta.get("description"),
        "url": meta.get("url") or url,
    }


def extract(html: str, url: str, output_format: str = "markdown") -> dict:
    """Full extraction for generic sites."""
    meta = trafilatura.bare_extraction(
        html,
        url=url,
        include_tables=True,
        include_comments=False,
        no_fallback=False,
        with_metadata=True,
        as_dict=True,
    )

    if meta is None:
        return {"success": False}

    text = trafilatura.extract(
        html,
        url=url,
        output_format=output_format,
        include_tables=True,
        include_comments=False,
        no_fallback=False,
    )

    return {
        "success": True,
        "markdown": text or "",
        "title": meta.get("title"),
        "authors": meta.get("author"),
        "date": meta.get("date"),
        "description": meta.get("description"),
        "url": meta.get("url") or url,
    }
