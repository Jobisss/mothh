import unittest

from yt_dlp import command


class WrapperTests(unittest.TestCase):
    def test_provider_options_apply_before_search_and_playback_arguments(self):
        for request in ("ytsearch1:artista música", "https://www.youtube.com/watch?v=abcdefghijk"):
            arguments = ["--ignore-config", "--output", "-", "--", request]
            argv = command(arguments, "http://music-pot:4416")
            self.assertEqual(argv[-len(arguments):], arguments)
            self.assertIn("youtube:player_client=mweb", argv)
            self.assertIn("youtubepot-bgutilhttp:base_url=http://music-pot:4416", argv)

    def test_empty_endpoint_preserves_default_client(self):
        self.assertEqual(command(["--version"], ""), ["/opt/yt-dlp/bin/yt-dlp", "--version"])


if __name__ == "__main__":
    unittest.main()
