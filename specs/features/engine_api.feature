Feature: The engine's interface
  The window is a web page and the engine is a local HTTP server, so this is the contract between
  them. Every refusal has to carry a sentence a person can act on, because the window shows it
  verbatim rather than inventing a reason of its own.

  Scenario: the capability report is small and to the point
    When the window asks what this machine can do
    Then the answer names the engine version
    And the answer says whether H.264 encoding is available
    And the answer does not carry the whole encoder list

  Scenario: a refusal names what is wrong
    When I ask about a file that is not there
    Then the answer is a refusal
    And the refusal names the file

  Scenario: a range against a file that is not there is refused, not raised
    When I plan a range against a file that is not there
    Then the answer is a refusal and not a traceback

  Scenario: one cut at a time
    Given a cut already running
    When I start another
    Then the second is refused because one is already running

  Scenario: cancelling nothing is not an error
    When I cancel with nothing running
    Then the answer is that nothing was running

  Scenario: the route list names the routes that exist
    When I ask the engine what it serves
    Then the route list is exactly the routes it has

  Scenario: a file outside the interface is not served
    When I ask for a page that does not exist
    Then the interface's own document is served

  Scenario: the window is told not to keep a copy of the interface
    When I ask for a page that does not exist
    Then the answer says the interface may not be stored
