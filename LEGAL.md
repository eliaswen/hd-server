# Legal notice

hd-server is an independent, clean-room server that speaks the network
protocol of *Hay Day* 1.43.150. It is published for interoperability, research
and preservation, under the MIT license and with no warranty (see
[LICENSE](LICENSE)).

This notice explains what the project does and does not contain, and what it
expects from you. It gives examples from US, French and EU law to show the
rules that usually matter for projects like this one. **It is not legal
advice.** Laws change, courts disagree, and the outcome depends on where you
live and what you actually do. If you have a real legal question, ask a lawyer
qualified where you live.

## What this repository contains

- **Original code only.** The server is written from scratch in Rust. It does
  not include or link any Supercell code.
- **No game files.** No APK, binaries, assets, or data tables are distributed.
  `resources/` is generated on your machine by
  [hd-dumper](https://github.com/eliaswen/hd-dumper) from a copy of the game
  you are entitled to use. It is not committed and not embedded in the
  executable.
- **No Supercell keys.** The server generates its own X25519 key pair. It does
  not contain or recover any key belonging to Supercell.
- **No client modification.** This repository does not patch the game. Pointing
  a client at this server requires a separate tool
  ([hd-patcher](https://github.com/eliaswen/hd-patcher)), which has its own
  notice and its own legal considerations.
- **No connection to Supercell.** The server never contacts Supercell's
  infrastructure and does not proxy or relay traffic to it.

## Your responsibilities

You are responsible for complying with the law that applies to you, and with
any agreement you have accepted. In particular:

- Only use game data extracted from a copy of the game you are entitled to use.
- Do not redistribute the generated `resources/`, captures in `dumps/`, or any
  other material derived from the game's files.
- Only redirect traffic from devices and networks you control. Answering
  for `gamea.haydaygame.com` (through DNS, a proxy, or a routed address) on
  someone else's network or device is interception, not interoperability.
- Do not use the server, or the knowledge gained from it, to access, probe or
  load-test Supercell's servers or any other system you are not authorized to
  use.
- Do not use it to obtain paid content without paying. Gold, diamonds and other
  balances granted by `--cash`, `--diamonds` or `dev_friends` exist only in
  your local save. They are not, and must not be presented as, purchases or
  items on the official game.
- Do not sell access to a server, sell in-game currency or items, or otherwise
  make money from the game through this project.

## Supercell Terms of Service

This project is not affiliated with, endorsed by, or sponsored by Supercell.

Playing on a third-party server requires a modified client, and the
[Supercell Terms of Service](https://supercell.com/en/terms-of-service/)
prohibit, among other things, reverse engineering or modifying the game client
and using unauthorized third-party software. Doing so may lead Supercell to
suspend or terminate your account and may expose you to other remedies under
those terms. Read the current version yourself, as it may change.

To limit that risk, never sign in to a modified client with your real
Supercell ID or official account, and never connect a modified client to
Supercell's own servers.

How far those terms can be enforced depends on where you are. See the
[Bowers](#united-states) and [Davidson v. Jung](#united-states) cases for the
US, and [Article 8 of the Software Directive](#european-union) and
[Article L122-6-1](#france) for the EU and France.

## Cheating

This project is not a cheating tool and must not be used as one. It answers a
client that you have chosen to point at your own server. Nothing it does reaches
the official game, other players, or their accounts.

You must not use it, or anything derived from it, to:

- cheat, exploit, or gain an unfair advantage in the official game or its
  community;
- disrupt, overload, or attack Supercell's servers or other players;
- bypass anti-cheat, anti-tamper, or other protection measures on services you
  do not operate.

For an example of what can happen when automation or circumvention touches an
official online game, see *MDY v. Blizzard* under
[United States](#united-states).

## Running a server for other people

hd-server is designed for a single person running it on their own machine. If
you let anyone else connect, you take on extra obligations.

- **Personal data.** The server stores account IDs and tokens
  (`data/accounts.json`), every turn received (`data/<id>/turns.jsonl`), full
  traffic captures (`dumps/server/`) and connection logs that may include IP
  addresses. Under the GDPR this is personal data (see
  [European Union](#european-union)). You need a legal basis to process it, you
  must tell users what you keep and for how long, and you must keep it secure.
- **The admin API has no authentication.** Keep it on loopback. Exposing it
  with `--admin-insecure` lets anyone with network access read and overwrite
  every account.
- **Login is not authentication.** The server accepts whatever ID and token a
  client presents. Do not let anyone rely on it to protect their data.
- **Commercial use.** Hosting a public or paid server is where game publishers
  are most likely to take action, and where the defenses discussed below are
  weakest (see *Blizzard v. Scapegaming* under
  [United States](#united-states)).

## Jurisdiction examples

The examples below are a summary only. Check the current text of each law and
how courts have applied it.

### European Union

- **Software Directive (2009/24/EC), Article 5(3).** A person entitled to use a
  program may observe, study and test how it works in order to understand the
  ideas and principles behind it, while running it in the normal way.
- **Software Directive, Article 6.** Decompilation is allowed without the
  rightholder's permission when it is necessary to obtain the information
  needed for the interoperability of an independently created program, the
  information is not already readily available, and only the necessary parts
  are decompiled. The information cannot be shared beyond what
  interoperability requires, or used to create a substantially similar
  program.
- **Software Directive, Article 8.** Contract terms that contradict Article 6
  or the exceptions in Article 5(2) and (3) are null and void. A licence or
  terms of service therefore cannot remove those permissions, although they
  may still restrict other conduct.
- ***SAS Institute v World Programming* (CJEU, C-406/10, 2012).** The
  functionality of a program, its programming language and its data file
  formats are not protected by copyright as such. Network message layouts
  and protocol behavior are, by analogy, generally treated as ideas rather
  than protected expression. The game's code, art, text and data tables are
  still protected, which is why this repository includes none of them.
- ***Nintendo v PC Box* (CJEU, C-355/12, 2014).** Video games are complex works
  protected not only as software but also through their graphic and sound
  elements, under the InfoSoc Directive (2001/29/EC). Protections against
  circumventing technical measures apply to those elements, assessed for
  proportionality.
- **Trade Secrets Directive (2016/943), Article 3(1)(b).** Studying or
  reverse engineering a product that has been made available to the public or
  lawfully acquired is a lawful way to obtain information, unless you are
  under a valid duty to limit that.
- **GDPR (Regulation 2016/679).** Accounts, turns, captures and IP addresses
  of other people are personal data. Article 2(2)(c) excludes purely personal
  or household activity, which covers running the server only for yourself.
  Hosting it for others does not fall under that exclusion.
- **EU Trade Mark Regulation (2017/1001), Article 14(1)(c).** A trademark owner
  cannot prevent the use of its mark to identify or refer to its own goods,
  when that use is in accordance with honest practices.

### France

- **Intellectual Property Code (CPI), Article L122-6-1 III.** A person entitled
  to use a program may observe, study or test how it works without permission,
  to determine its underlying ideas and principles.
- **CPI, Article L122-6-1 IV.** This implements Article 6 of the Software
  Directive. Reproducing or translating a program's code is allowed when it is
  necessary to obtain information needed for the interoperability of an
  independently created program, under the same conditions: done by a person
  entitled to use the program, limited to the necessary parts, using
  information not already readily available, not disclosed except as needed
  for interoperability, and not used to make a substantially similar program
  or for any act that infringes copyright.
- **CPI, Article L122-6-1 (final paragraph).** Any contract clause contrary to
  sections II, III and IV is void (*nulle et non avenue*).
- **CPI, Article L122-6-2.** Unlawfully using a means of removing or
  neutralizing a technical device that protects software is punishable as
  infringement (*contrefaçon*, Articles L335-2 and L335-3). This matters mostly
  for client-modification tools, not for this server.
- **Penal Code, Articles 323-1 and 323-2.** Fraudulently accessing or staying
  in an automated data processing system (*STAD*), or hindering its operation,
  is a criminal offense. Do not use this project or its findings against
  Supercell's servers or any system you are not authorized to use.
- **CPI, Article L713-6.** A trademark may be used where necessary to indicate
  what a product or service is intended for, provided that use is in line with
  honest practices in industry or commerce.
- **Loi Informatique et Libertés and the CNIL.** France applies the GDPR
  through this law, enforced by the CNIL, if you host the server for others.

### United States

- **Fair use, 17 U.S.C. § 107.** In *Sega v. Accolade* (9th Cir. 1992) and
  *Sony v. Connectix* (9th Cir. 2000), disassembling a program to learn the
  unprotected elements needed for compatibility was held to be fair use, when
  it was the only way to gain access and there was a legitimate reason for
  seeking it.
- **DMCA, 17 U.S.C. § 1201(f).** Circumventing a technological measure is
  allowed for the sole purpose of identifying and analyzing the elements
  needed for the interoperability of an independently created program, to the
  extent that doing so is not otherwise infringing.
- **DMCA exemptions, 37 C.F.R. § 201.40.** The Librarian of Congress grants
  temporary exemptions every three years. The video game exemption allows
  circumvention to keep playing a game only after the copyright owner has
  stopped providing the server support it needs. *Hay Day*'s official servers
  are still running, so it does not cover this game today.
- ***Davidson & Associates v. Jung* (8th Cir. 2005).** Developers of *bnetd*, a
  compatible server for Blizzard's Battle.net, were held liable for breaching
  the EULA's ban on reverse engineering and for violating the DMCA's
  anti-circumvention rules. The court did not apply the § 1201(f) exemption,
  partly because *bnetd* let copies run without a valid CD key check. This is
  the closest US precedent for a project like this one, and the main reason it
  contains no code that bypasses licensing or purchase checks.
- ***Bowers v. Baystate Technologies* (Fed. Cir. 2003).** A contractual ban on
  reverse engineering was held enforceable and not preempted by copyright law.
  Unlike in the EU, US terms of service can therefore remove permissions you
  would otherwise have.
- ***MDY Industries v. Blizzard Entertainment* (9th Cir. 2010).** The maker of
  *Glider*, a bot for *World of Warcraft*, was held liable under § 1201(a)(2)
  for trafficking in a tool that evaded the game's anti-cheat system.
- ***Blizzard Entertainment v. Scapegaming* (C.D. Cal. 2010).** The operator of
  a monetized *World of Warcraft* private server was ordered to pay about
  US$88 million in a default judgment for copyright infringement and DMCA
  violations.
- **Computer Fraud and Abuse Act, 18 U.S.C. § 1030.** Accessing a computer
  without authorization is a federal offense. *Van Buren v. United States*
  (2021) narrowed "exceeds authorized access" but did not change the rule
  against accessing systems you have no permission to use at all.
- **Nominative fair use.** In *New Kids on the Block v. News America
  Publishing* (9th Cir. 1992), using a trademark to refer to the owner's own
  product was allowed when it was needed to identify that product, used only
  as much as necessary, and did not suggest sponsorship or endorsement.

## Trademarks

"Hay Day" and "Supercell" are trademarks of their respective owners. They are
used here only to describe which game this server is compatible with. Their use
does not imply any affiliation with, or endorsement by, Supercell.

## Contact

If you are a rightholder and believe something in this repository infringes
your rights, open an issue or contact the maintainer, and it will be reviewed
promptly.
