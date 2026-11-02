#!/usr/bin/python3
"""Opções de implantação compartilhadas pela busca e pelo worker."""
import os
import sys


def command(arguments, provider_url):
    options = []
    if provider_url:
        options = [
            "--extractor-args", "youtube:player_client=mweb",
            "--extractor-args", f"youtubepot-bgutilhttp:base_url={provider_url}",
        ]
    return ["/opt/yt-dlp/bin/yt-dlp", *options, *arguments]


if __name__ == "__main__":
    argv = command(sys.argv[1:], os.environ.get("MOTHH_MUSIC_POT_URL", ""))
    os.execv(argv[0], argv)
