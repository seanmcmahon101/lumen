# Privacy

Lumen searches applications, files, spreadsheet text and locally available
Outlook messages on your Windows PC. Indexing and ranking happen locally.
Lumen does not upload your files, workbook contents, email contents, search
history or search terms to a Lumen server or signing provider. It does not
include advertising or usage analytics.

To choose folders worth indexing, Lumen reads the Windows jump lists on your PC
(the recent and pinned items behind Quick Access and taskbar menus). It uses
only item paths, open counts and dates, saves only the folders it adds to your
settings, and does not change or upload the jump lists. Turning off
**Default locations** in `/settings` stops this.

Lumen checks this project's public GitHub releases for updates at startup and
periodically while it runs. GitHub receives the normal network information
needed to serve that request, including your IP address; your search terms and
indexed contents are not included. Clicking an update opens its GitHub download
in your browser. See the
[GitHub privacy statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement).

Local Outlook search uses Windows Search and, where available, a running classic
Outlook session. Choosing **Search Outlook online** opens your query in Microsoft
365 in your browser. Only that explicit action sends the query to Microsoft;
your browser's existing sign-in applies. See the
[Microsoft privacy statement](https://privacy.microsoft.com/privacystatement).

Opening a result uses the app or service associated with it on your PC. That
app's privacy practices apply. Lumen's code-signing service receives release
build artifacts during development, not data from people using Lumen.
