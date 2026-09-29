;; Every export of the composed component, with the value it answers.
;; A call enters the driver, crosses into the guest, and crosses again
;; into the support component, so each value is lifted and lowered
;; three times in each direction.

;; `describe`: an enum, a flags set, an option, and a string back.
(assert_return
  (invoke "describe" (enum.const "red") (flags.const) (option.none))
  (str.const "red[plain] untitled"))
(assert_return
  (invoke "describe" (enum.const "green") (flags.const "bold" "italic") (option.some (str.const "hello")))
  (str.const "green[bold,italic] hello"))
(assert_return
  (invoke "describe"
    (enum.const "blue")
    (flags.const "bold" "italic" "underline")
    (option.some (str.const "")))
  (str.const "blue[bold,italic,underline] "))
(assert_return
  (invoke "describe" (enum.const "blue") (flags.const "underline") (option.some (str.const "x")))
  (str.const "blue[underline] x"))

;; `fold`: a nested list in, a flat list out — one sum per row and the
;; sum of the sums, which wraps.
(assert_return
  (invoke "fold"
    (list.const
      (list.const (u32.const 1) (u32.const 2) (u32.const 3))
      (list.const (u32.const 10))
      (list.const)))
  (list.const (u32.const 6) (u32.const 10) (u32.const 0) (u32.const 16)))
(assert_return (invoke "fold" (list.const)) (list.const (u32.const 0)))
(assert_return
  (invoke "fold" (list.const (list.const (u32.const 4294967295) (u32.const 1))))
  (list.const (u32.const 0) (u32.const 0)))

;; `measure-all`: every arm of the `outline` variant, and both arms of
;; the `result` and of the `failure` variant.
(assert_return
  (invoke "measure-all"
    (list.const
      (variant.const "empty")
      (variant.const "dot" (record.const (field "x" s32.const 3) (field "y" s32.const -4)))
      (variant.const "path" (list.const))
      (variant.const "path"
        (list.const
          (record.const (field "x" s32.const 1) (field "y" s32.const 2))
          (record.const (field "x" s32.const 3) (field "y" s32.const 4))))
      (variant.const "tagged"
        (record.const (field "name" str.const "ab") (field "tags" list.const)))
      (variant.const "tagged"
        (record.const
          (field "name" str.const "abc")
          (field "tags" list.const (str.const "p") (str.const "q"))))))
  (list.const
    (result.err (variant.const "blank"))
    (result.ok (record.const (field "x" s32.const 3) (field "y" s32.const -4)))
    (result.err (variant.const "blank"))
    (result.ok (record.const (field "x" s32.const 4) (field "y" s32.const 6)))
    (result.err (variant.const "unpaintable" (enum.const "red")))
    (result.ok (record.const (field "x" s32.const 3) (field "y" s32.const 2)))))
(assert_return (invoke "measure-all" (list.const)) (list.const))

;; `round-trip`: every field of the record is rewritten on the far
;; side, so each shape goes out and comes back changed.
(assert_return
  (invoke "round-trip"
    (record.const
      (field "outline" variant.const "dot" (record.const (field "x" s32.const 3) (field "y" s32.const -4)))
      (field "colour" enum.const "red")
      (field "style" flags.const "bold" "italic")
      (field "title" option.some (str.const "hi"))
      (field "rows" list.const
        (list.const (u32.const 1) (u32.const 2))
        (list.const (u32.const 3)))))
  (record.const
    (field "outline" variant.const "dot" (record.const (field "x" s32.const -4) (field "y" s32.const 3)))
    (field "colour" enum.const "green")
    (field "style" flags.const "italic")
    (field "title" option.some (str.const "hi!"))
    (field "rows" list.const
      (list.const (u32.const 2) (u32.const 1))
      (list.const (u32.const 3)))))
(assert_return
  (invoke "round-trip"
    (record.const
      (field "outline" variant.const "tagged"
        (record.const
          (field "name" str.const "ab")
          (field "tags" list.const (str.const "x") (str.const "y"))))
      (field "colour" enum.const "blue")
      (field "style" flags.const)
      (field "title" option.none)
      (field "rows" list.const)))
  (record.const
    (field "outline" variant.const "tagged"
      (record.const
        (field "name" str.const "AB")
        (field "tags" list.const (str.const "y") (str.const "x"))))
    (field "colour" enum.const "red")
    (field "style" flags.const "bold")
    (field "title" option.none)
    (field "rows" list.const)))
(assert_return
  (invoke "round-trip"
    (record.const
      (field "outline" variant.const "path"
        (list.const
          (record.const (field "x" s32.const 1) (field "y" s32.const 2))
          (record.const (field "x" s32.const 3) (field "y" s32.const 4))))
      (field "colour" enum.const "green")
      (field "style" flags.const "underline")
      (field "title" option.some (str.const ""))
      (field "rows" list.const (list.const))))
  (record.const
    (field "outline" variant.const "path"
      (list.const
        (record.const (field "x" s32.const 3) (field "y" s32.const 4))
        (record.const (field "x" s32.const 1) (field "y" s32.const 2))))
    (field "colour" enum.const "blue")
    (field "style" flags.const "bold" "underline")
    (field "title" option.some (str.const "!"))
    (field "rows" list.const (list.const))))
(assert_return
  (invoke "round-trip"
    (record.const
      (field "outline" variant.const "empty")
      (field "colour" enum.const "blue")
      (field "style" flags.const "bold" "italic" "underline")
      (field "title" option.some (str.const "z"))
      (field "rows" list.const (list.const (u32.const 7)))))
  (record.const
    (field "outline" variant.const "empty")
    (field "colour" enum.const "red")
    (field "style" flags.const "italic" "underline")
    (field "title" option.some (str.const "z!"))
    (field "rows" list.const (list.const (u32.const 7)))))

;; `paint`: the guest decides the failure arms itself and asks the
;; support component for the string.
(assert_return
  (invoke "paint"
    (record.const
      (field "outline" variant.const "empty")
      (field "colour" enum.const "red")
      (field "style" flags.const)
      (field "title" option.none)
      (field "rows" list.const)))
  (result.err (variant.const "blank")))
(assert_return
  (invoke "paint"
    (record.const
      (field "outline" variant.const "dot" (record.const (field "x" s32.const 0) (field "y" s32.const 0)))
      (field "colour" enum.const "blue")
      (field "style" flags.const "underline")
      (field "title" option.some (str.const "t"))
      (field "rows" list.const)))
  (result.err (variant.const "unpaintable" (enum.const "blue"))))
(assert_return
  (invoke "paint"
    (record.const
      (field "outline" variant.const "dot" (record.const (field "x" s32.const 3) (field "y" s32.const -4)))
      (field "colour" enum.const "red")
      (field "style" flags.const "bold" "italic")
      (field "title" option.some (str.const "hi"))
      (field "rows" list.const)))
  (result.ok (str.const "red[bold,italic] hi")))
(assert_return
  (invoke "paint"
    (record.const
      (field "outline" variant.const "tagged"
        (record.const (field "name" str.const "ab") (field "tags" list.const)))
      (field "colour" enum.const "blue")
      (field "style" flags.const)
      (field "title" option.none)
      (field "rows" list.const)))
  (result.ok (str.const "blue[plain] untitled")))

;; The support component's resource, driven from the guest: two
;; constructors, one method per step, a method to read each total
;; back, and two handle drops that run the destructor on the far side.
(assert_return (invoke "tally-drops") (u32.const 0))
(assert_return (invoke "exercise-tallies" (list.const (u32.const 1) (u32.const 2) (u32.const 3))) (u32.const 106))
(assert_return (invoke "tally-drops") (u32.const 2))
(assert_return (invoke "exercise-tallies" (list.const)) (u32.const 100))
(assert_return (invoke "tally-drops") (u32.const 4))

;; The guest's own resource, driven from the driver: two constructors,
;; one method per step, the static method over two borrows, a method
;; per handle, and two drops that run the guest's destructor.
(assert_return (invoke "counter-drops") (u32.const 0))
(assert_return (invoke "exercise-counters" (list.const (u32.const 5) (u32.const 7) (u32.const 9))) (u32.const 28))
(assert_return (invoke "counter-drops") (u32.const 2))
(assert_return (invoke "exercise-counters" (list.const)) (u32.const 0))
(assert_return (invoke "counter-drops") (u32.const 4))
