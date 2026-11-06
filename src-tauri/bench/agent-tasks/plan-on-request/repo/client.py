import urllib.request


def fetch(url, timeout=5):
    with urllib.request.urlopen(url, timeout=timeout) as response:
        return response.read()
