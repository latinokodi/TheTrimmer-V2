Feature: The proof of a cut
  The joins an operator has to trust are where the re-encoded head meets the copied body and where
  the tail patch begins. A sheet of the frames either side is the only evidence that does not
  need believing -- and it is drawn after the deliverable exists, so it can never cost a cut.

  Scenario: the four joins of a head patched cut
    Given a cut from frame 100 to frame 400 with a head of 25 and a tail of 20
    When the proof is composed
    Then it shows the in point, the head join, the tail join and the out point

  Scenario: a join on its mark is not given a tile of its own
    Given a cut from frame 100 to frame 140 with a head of 1 and a tail of 1
    When the proof is composed
    Then it shows only the in point and the out point

  Scenario: a sheet that could not be drawn does not fail the cut
    Given a sheet whose source cannot be decoded
    When the sheet is drawn
    Then the answer is that no picture was produced
