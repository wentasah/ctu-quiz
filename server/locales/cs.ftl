# Czech texts of the web UI. See en.ftl.

## All pages

quiz = Kvíz
page-title = Kvíz – otázka { $number }
logout = odhlásit
offline = Připojení přerušeno, obnovuji…
back-home = Zpět na hlavní stránku

## The main page

logged-in-as = Přihlášen jako <strong>{ $name }</strong> ({ $login })
question-code = Kód otázky
question-code-hint = Zadejte kód, který vyučující zobrazil vedle QR kódu.
show-question = Zobrazit otázku
unknown-code = Kód { $code } nepatří k žádné otázce. Zkuste to prosím znovu.
source-code = Zdrojový kód:
language = Jazyk
change-language = Změnit

## The student's page

wait-for-question = Čekejte na otázku
wait-for-question-hint =
    Až vyučující položí otázku, objeví se zde sama.
    Tuto stránku nezavírejte.
room-code = Kód této místnosti: <strong>{ $code }</strong>
question-on-screen = Otázka je jen na plátně.
options-on-screen = Možnosti jsou na plátně; vyberte písmeno své odpovědi.
answer-recorded =
    Vaše odpověď <strong>{ $label }</strong> byla zaznamenána.
    Dokud je otázka otevřená, můžete ji změnit.
choose-answer = Vyberte jednu z možností.
students-answered =
    Odpovědělo <strong data-count>{ $count }</strong>
    <span class="unit">studentů</span>
question-is-closed = Otázka je uzavřena
answer-correct = Vaše odpověď <strong>{ $label }</strong> byla správná.
answer-partial = Vaše odpověď <strong>{ $label }</strong> byla částečně správná.
answer-wrong = Vaše odpověď <strong>{ $label }</strong> byla špatná.
your-answer-was = Vaše odpověď byla <strong>{ $label }</strong>.
not-answered = Na tuto otázku jste neodpověděl(a).
next-question-here = Další otázka se zde objeví sama, stránku nezavírejte.

## The presentation view

or-type-code = nebo zadejte na <strong>{ $host }</strong> kód
answered = Odpovědělo
close-question = Uzavřít otázku
question-closed = Otázka uzavřena
reveal-answers = Ukázat správné odpovědi

## Results

correct-answer = správná odpověď
total-answers = Celkem odpovědí: <strong data-count>{ $count }</strong>

## Errors

not-found = Nenalezeno
bad-request = Chybný požadavek
forbidden = Nepovoleno
server-error = Chyba serveru
server-error-hint = Požadavek se nepodařilo zpracovat. Zkuste to prosím znovu.
page-gone = Tato stránka na serveru není, nebo už byla odstraněna.
question-gone = Tato otázka už na serveru není.
question-not-accepting = Otázka už nepřijímá odpovědi.
no-such-answer = Taková odpověď neexistuje.
no-room = Nemáte otevřenou žádnou místnost. Otázku položíte příkazem quiz ask.
not-a-teacher = Tato stránka je určena pro projektor vyučujícího. Na otázky odpovídejte na stránce, na kterou vás dovedl QR kód nebo kód z projektoru. Pokud jste vyučující, položte nejdříve otázku příkazem quiz ask.
server-busy = Server je zaneprázdněn
too-many-logins = Právě probíhá příliš mnoho přihlášení. Zkuste to prosím za chvíli znovu.
login-failed = Přihlášení se nezdařilo
login-error = Přihlášení se nezdařilo: { $error } { $description }
login-incomplete = Odpověď od poskytovatele identity je neúplná.
login-other-browser = Přihlášení nebylo zahájeno v tomto prohlížeči. Zkuste to prosím znovu.
login-expired = Přihlášení vypršelo, zkuste to prosím znovu.
login-refused = Poskytovatel identity odmítl přihlášení.
