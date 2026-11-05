# English texts of the web UI.
#
# Messages whose text contains HTML markup are inserted into the pages as
# is; their variables are escaped. Every message has to be in cs.ftl as well.

## All pages

quiz = Quiz
page-title = Quiz – question { $number }
logout = log out
offline = Connection lost, reconnecting…
back-home = Back to the main page

## The main page

logged-in-as = Logged in as <strong>{ $name }</strong> ({ $login })
question-code = Question code
question-code-hint = Enter the code the teacher shows next to the QR code.
show-question = Show the question
unknown-code = Code { $code } does not belong to any question. Please try again.
source-code = Source code:
language = Language
change-language = Change

## The student's page

wait-for-question = Wait for a question
wait-for-question-hint =
    It appears here by itself once the teacher asks it.
    Do not close this page.
room-code = Code of this room: <strong>{ $code }</strong>
question-on-screen = The question is on the screen only.
options-on-screen = The options are on the screen; choose the letter of your answer.
answer-recorded =
    Your answer <strong>{ $label }</strong> has been recorded.
    You can change it while the question is open.
choose-answer = Choose one of the options.
students-answered =
    Answered by <strong data-count>{ $count }</strong>
    <span class="unit">students</span>
question-is-closed = The question is closed
answer-correct = Your answer <strong>{ $label }</strong> was correct.
answer-partial = Your answer <strong>{ $label }</strong> was partially correct.
answer-wrong = Your answer <strong>{ $label }</strong> was wrong.
your-answer-was = Your answer was <strong>{ $label }</strong>.
not-answered = You did not answer this question.
next-question-here = The next question appears here by itself; do not close this page.

## The presentation view

or-type-code = or enter the code at <strong>{ $host }</strong>
answered = Answered
close-question = Close the question
question-closed = Question closed
reveal-answers = Show the correct answers

## Results

correct-answer = correct answer
total-answers = Answers in total: <strong data-count>{ $count }</strong>

## Errors

not-found = Not found
bad-request = Bad request
forbidden = Not allowed
server-error = Server error
server-error-hint = The request could not be processed. Please try again.
page-gone = This page is not on the server, or it has been removed.
question-gone = This question is no longer on the server.
question-not-accepting = The question no longer accepts answers.
no-such-answer = There is no such answer.
no-room = You have no room open. Ask a question with quiz ask.
not-a-teacher = This page is for the teacher's projector. Answer questions on the page you reached with the QR code or the code on the projector. If you are the teacher, ask a question with quiz ask first.
server-busy = Server busy
too-many-logins = Too many logins are in progress. Please try again in a moment.
login-failed = Login failed
login-error = Login failed: { $error } { $description }
login-incomplete = The response of the identity provider is incomplete.
login-other-browser = The login was not started in this browser. Please try again.
login-expired = The login has expired, please try again.
login-refused = The identity provider refused the login.
