# Permission request to send Rolimons

Send via Discord (https://discord.gg/rolimons) or X (@rolimons) — their contact
page lists no email address. Keep it short; you want a yes or a no, not a
discussion.

---

Hi! I build RoJoin, a free open-source desktop Roblox launcher
(github.com/ikottgicki-rgb/rojoin, MIT).

I'd like to show a game's player-count history on its page in the app, and
Rolimons is the only place that has it — Roblox doesn't expose any history to a
client. I know your terms say no automated access or redisplay, so I've taken
that out and I'm asking rather than assuming.

What I'd want to do:
- fetch a game page only when a user opens that game's History tab, never in
  bulk and never on a schedule
- cache it so repeat views don't re-request
- honour the 2s crawl-delay from your robots.txt
- identify the app honestly in the User-Agent, with a link back to the repo, so
  you can see and block it if it ever becomes a nuisance
- credit Rolimons visibly on the chart, with a link through to your page for the
  full history

No commercial use — the app is free and I'm not reselling data or building
anything that competes with you.

Is that something you'd allow? Happy to work to whatever limits you'd prefer, or
to drop it entirely if the answer is no.

---

## If they say yes

The implementation still exists in git history:

    git revert dccd5df     # restores the Rolimons fetch + History tab wiring

That commit removed it. The version it restores already had the honest
User-Agent and the 2-second crawl-delay, so only the attribution wording would
need a look.

## If they say no, or don't reply

Current behaviour stands: RoJoin records its own samples (players, visits, votes)
from the public Roblox endpoints the game page already calls, and links out to
Rolimons for the full history.
