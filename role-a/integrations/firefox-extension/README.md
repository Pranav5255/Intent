# Intent Firefox companion

The companion sends sanitized active-tab metadata to the local Event API. Its
optional content script additionally records semantic user actions after
`intent-osctl detailed browser enable` is run: clicks, links, form submissions,
checkbox/radio toggles, select changes, and throttled page scrolls. Scroll
events contain only direction and a 0-10 page-position bucket.

It does not collect typed form values, keystrokes, clipboard contents, page HTML,
DOM snapshots, network traffic, or pointer coordinates. Private tabs create no
detailed events. Login, account, billing, checkout, password, and payment pages
emit only click/submit tag-and-role metadata with no labels or destinations.

`npm test` runs sanitization and payload checks. `npm run build` creates an
unsigned development XPI that includes `content.js`; release packaging still
requires a newly Mozilla-signed XPI after this extension source changes.
