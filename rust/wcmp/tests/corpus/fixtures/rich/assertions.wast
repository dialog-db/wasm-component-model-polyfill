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

;; `mix`: nineteen flat parameters, so each caller spills them to
;; memory, carrying every scalar type, a tuple, a nested option, a set
;; of thirty-two flags, a string of multi-byte characters, and a bulk
;; list of bytes. The support component changes each value, so each
;; one is checked going out and coming back.
;; Each type at its edge, and every byte value.
(assert_return
  (invoke "mix"
    (record.const
      (field "letter" char.const "é")
      (field "flag" bool.const true)
      (field "short" u16.const 65535)
      (field "big" u64.const 18446744073709551615)
      (field "signed" s64.const -9223372036854775808)
      (field "single" f32.const 1.5)
      (field "double" f64.const 10)
      (field "pair" tuple.const (u8.const 255) (s16.const -32768))
      (field "nested" option.none)
      (field "marks" flags.const "f1" "f31"))
    (str.const "héllo wörld ✓🎉")
    (list.const (u8.const 0) (u8.const 1) (u8.const 2) (u8.const 3) (u8.const 4)
      (u8.const 5) (u8.const 6) (u8.const 7) (u8.const 8) (u8.const 9)
      (u8.const 10) (u8.const 11) (u8.const 12) (u8.const 13) (u8.const 14)
      (u8.const 15) (u8.const 16) (u8.const 17) (u8.const 18) (u8.const 19)
      (u8.const 20) (u8.const 21) (u8.const 22) (u8.const 23) (u8.const 24)
      (u8.const 25) (u8.const 26) (u8.const 27) (u8.const 28) (u8.const 29)
      (u8.const 30) (u8.const 31) (u8.const 32) (u8.const 33) (u8.const 34)
      (u8.const 35) (u8.const 36) (u8.const 37) (u8.const 38) (u8.const 39)
      (u8.const 40) (u8.const 41) (u8.const 42) (u8.const 43) (u8.const 44)
      (u8.const 45) (u8.const 46) (u8.const 47) (u8.const 48) (u8.const 49)
      (u8.const 50) (u8.const 51) (u8.const 52) (u8.const 53) (u8.const 54)
      (u8.const 55) (u8.const 56) (u8.const 57) (u8.const 58) (u8.const 59)
      (u8.const 60) (u8.const 61) (u8.const 62) (u8.const 63) (u8.const 64)
      (u8.const 65) (u8.const 66) (u8.const 67) (u8.const 68) (u8.const 69)
      (u8.const 70) (u8.const 71) (u8.const 72) (u8.const 73) (u8.const 74)
      (u8.const 75) (u8.const 76) (u8.const 77) (u8.const 78) (u8.const 79)
      (u8.const 80) (u8.const 81) (u8.const 82) (u8.const 83) (u8.const 84)
      (u8.const 85) (u8.const 86) (u8.const 87) (u8.const 88) (u8.const 89)
      (u8.const 90) (u8.const 91) (u8.const 92) (u8.const 93) (u8.const 94)
      (u8.const 95) (u8.const 96) (u8.const 97) (u8.const 98) (u8.const 99)
      (u8.const 100) (u8.const 101) (u8.const 102) (u8.const 103) (u8.const 104)
      (u8.const 105) (u8.const 106) (u8.const 107) (u8.const 108) (u8.const 109)
      (u8.const 110) (u8.const 111) (u8.const 112) (u8.const 113) (u8.const 114)
      (u8.const 115) (u8.const 116) (u8.const 117) (u8.const 118) (u8.const 119)
      (u8.const 120) (u8.const 121) (u8.const 122) (u8.const 123) (u8.const 124)
      (u8.const 125) (u8.const 126) (u8.const 127) (u8.const 128) (u8.const 129)
      (u8.const 130) (u8.const 131) (u8.const 132) (u8.const 133) (u8.const 134)
      (u8.const 135) (u8.const 136) (u8.const 137) (u8.const 138) (u8.const 139)
      (u8.const 140) (u8.const 141) (u8.const 142) (u8.const 143) (u8.const 144)
      (u8.const 145) (u8.const 146) (u8.const 147) (u8.const 148) (u8.const 149)
      (u8.const 150) (u8.const 151) (u8.const 152) (u8.const 153) (u8.const 154)
      (u8.const 155) (u8.const 156) (u8.const 157) (u8.const 158) (u8.const 159)
      (u8.const 160) (u8.const 161) (u8.const 162) (u8.const 163) (u8.const 164)
      (u8.const 165) (u8.const 166) (u8.const 167) (u8.const 168) (u8.const 169)
      (u8.const 170) (u8.const 171) (u8.const 172) (u8.const 173) (u8.const 174)
      (u8.const 175) (u8.const 176) (u8.const 177) (u8.const 178) (u8.const 179)
      (u8.const 180) (u8.const 181) (u8.const 182) (u8.const 183) (u8.const 184)
      (u8.const 185) (u8.const 186) (u8.const 187) (u8.const 188) (u8.const 189)
      (u8.const 190) (u8.const 191) (u8.const 192) (u8.const 193) (u8.const 194)
      (u8.const 195) (u8.const 196) (u8.const 197) (u8.const 198) (u8.const 199)
      (u8.const 200) (u8.const 201) (u8.const 202) (u8.const 203) (u8.const 204)
      (u8.const 205) (u8.const 206) (u8.const 207) (u8.const 208) (u8.const 209)
      (u8.const 210) (u8.const 211) (u8.const 212) (u8.const 213) (u8.const 214)
      (u8.const 215) (u8.const 216) (u8.const 217) (u8.const 218) (u8.const 219)
      (u8.const 220) (u8.const 221) (u8.const 222) (u8.const 223) (u8.const 224)
      (u8.const 225) (u8.const 226) (u8.const 227) (u8.const 228) (u8.const 229)
      (u8.const 230) (u8.const 231) (u8.const 232) (u8.const 233) (u8.const 234)
      (u8.const 235) (u8.const 236) (u8.const 237) (u8.const 238) (u8.const 239)
      (u8.const 240) (u8.const 241) (u8.const 242) (u8.const 243) (u8.const 244)
      (u8.const 245) (u8.const 246) (u8.const 247) (u8.const 248) (u8.const 249)
      (u8.const 250) (u8.const 251) (u8.const 252) (u8.const 253) (u8.const 254)
      (u8.const 255))
    (u32.const 7)
    (s8.const -128))
  (record.const
    (field "values" record.const
      (field "letter" char.const "ê")
      (field "flag" bool.const false)
      (field "short" u16.const 0)
      (field "big" u64.const 0)
      (field "signed" s64.const -9223372036854775808)
      (field "single" f32.const 3)
      (field "double" f64.const 2.5)
      (field "pair" tuple.const (u8.const 0) (s16.const -32768))
      (field "nested" option.some (option.none))
      (field "marks" flags.const "f0" "f1"))
    (field "text" str.const "🎉✓ dlröw olléh")
    (field "bytes" list.const (u8.const 255) (u8.const 254) (u8.const 253)
      (u8.const 252) (u8.const 251) (u8.const 250) (u8.const 249) (u8.const 248)
      (u8.const 247) (u8.const 246) (u8.const 245) (u8.const 244) (u8.const 243)
      (u8.const 242) (u8.const 241) (u8.const 240) (u8.const 239) (u8.const 238)
      (u8.const 237) (u8.const 236) (u8.const 235) (u8.const 234) (u8.const 233)
      (u8.const 232) (u8.const 231) (u8.const 230) (u8.const 229) (u8.const 228)
      (u8.const 227) (u8.const 226) (u8.const 225) (u8.const 224) (u8.const 223)
      (u8.const 222) (u8.const 221) (u8.const 220) (u8.const 219) (u8.const 218)
      (u8.const 217) (u8.const 216) (u8.const 215) (u8.const 214) (u8.const 213)
      (u8.const 212) (u8.const 211) (u8.const 210) (u8.const 209) (u8.const 208)
      (u8.const 207) (u8.const 206) (u8.const 205) (u8.const 204) (u8.const 203)
      (u8.const 202) (u8.const 201) (u8.const 200) (u8.const 199) (u8.const 198)
      (u8.const 197) (u8.const 196) (u8.const 195) (u8.const 194) (u8.const 193)
      (u8.const 192) (u8.const 191) (u8.const 190) (u8.const 189) (u8.const 188)
      (u8.const 187) (u8.const 186) (u8.const 185) (u8.const 184) (u8.const 183)
      (u8.const 182) (u8.const 181) (u8.const 180) (u8.const 179) (u8.const 178)
      (u8.const 177) (u8.const 176) (u8.const 175) (u8.const 174) (u8.const 173)
      (u8.const 172) (u8.const 171) (u8.const 170) (u8.const 169) (u8.const 168)
      (u8.const 167) (u8.const 166) (u8.const 165) (u8.const 164) (u8.const 163)
      (u8.const 162) (u8.const 161) (u8.const 160) (u8.const 159) (u8.const 158)
      (u8.const 157) (u8.const 156) (u8.const 155) (u8.const 154) (u8.const 153)
      (u8.const 152) (u8.const 151) (u8.const 150) (u8.const 149) (u8.const 148)
      (u8.const 147) (u8.const 146) (u8.const 145) (u8.const 144) (u8.const 143)
      (u8.const 142) (u8.const 141) (u8.const 140) (u8.const 139) (u8.const 138)
      (u8.const 137) (u8.const 136) (u8.const 135) (u8.const 134) (u8.const 133)
      (u8.const 132) (u8.const 131) (u8.const 130) (u8.const 129) (u8.const 128)
      (u8.const 127) (u8.const 126) (u8.const 125) (u8.const 124) (u8.const 123)
      (u8.const 122) (u8.const 121) (u8.const 120) (u8.const 119) (u8.const 118)
      (u8.const 117) (u8.const 116) (u8.const 115) (u8.const 114) (u8.const 113)
      (u8.const 112) (u8.const 111) (u8.const 110) (u8.const 109) (u8.const 108)
      (u8.const 107) (u8.const 106) (u8.const 105) (u8.const 104) (u8.const 103)
      (u8.const 102) (u8.const 101) (u8.const 100) (u8.const 99) (u8.const 98)
      (u8.const 97) (u8.const 96) (u8.const 95) (u8.const 94) (u8.const 93)
      (u8.const 92) (u8.const 91) (u8.const 90) (u8.const 89) (u8.const 88)
      (u8.const 87) (u8.const 86) (u8.const 85) (u8.const 84) (u8.const 83)
      (u8.const 82) (u8.const 81) (u8.const 80) (u8.const 79) (u8.const 78)
      (u8.const 77) (u8.const 76) (u8.const 75) (u8.const 74) (u8.const 73)
      (u8.const 72) (u8.const 71) (u8.const 70) (u8.const 69) (u8.const 68)
      (u8.const 67) (u8.const 66) (u8.const 65) (u8.const 64) (u8.const 63)
      (u8.const 62) (u8.const 61) (u8.const 60) (u8.const 59) (u8.const 58)
      (u8.const 57) (u8.const 56) (u8.const 55) (u8.const 54) (u8.const 53)
      (u8.const 52) (u8.const 51) (u8.const 50) (u8.const 49) (u8.const 48)
      (u8.const 47) (u8.const 46) (u8.const 45) (u8.const 44) (u8.const 43)
      (u8.const 42) (u8.const 41) (u8.const 40) (u8.const 39) (u8.const 38)
      (u8.const 37) (u8.const 36) (u8.const 35) (u8.const 34) (u8.const 33)
      (u8.const 32) (u8.const 31) (u8.const 30) (u8.const 29) (u8.const 28)
      (u8.const 27) (u8.const 26) (u8.const 25) (u8.const 24) (u8.const 23)
      (u8.const 22) (u8.const 21) (u8.const 20) (u8.const 19) (u8.const 18)
      (u8.const 17) (u8.const 16) (u8.const 15) (u8.const 14) (u8.const 13)
      (u8.const 12) (u8.const 11) (u8.const 10) (u8.const 9) (u8.const 8)
      (u8.const 7) (u8.const 6) (u8.const 5) (u8.const 4) (u8.const 3)
      (u8.const 2) (u8.const 1) (u8.const 0))
    (field "tag" u32.const 8)
    (field "step" s8.const -128)))

;; A character past the basic plane, an integer past 2^53, and empties.
(assert_return
  (invoke "mix"
    (record.const
      (field "letter" char.const "🎉")
      (field "flag" bool.const false)
      (field "short" u16.const 0)
      (field "big" u64.const 9007199254740993)
      (field "signed" s64.const 5)
      (field "single" f32.const -0.25)
      (field "double" f64.const -1e300)
      (field "pair" tuple.const (u8.const 0) (s16.const 1))
      (field "nested" option.some (option.none))
      (field "marks" flags.const))
    (str.const "日本語")
    (list.const)
    (u32.const 4294967295)
    (s8.const 3))
  (record.const
    (field "values" record.const
      (field "letter" char.const "🎊")
      (field "flag" bool.const true)
      (field "short" u16.const 1)
      (field "big" u64.const 9007199254740994)
      (field "signed" s64.const -5)
      (field "single" f32.const -0.5)
      (field "double" f64.const -2.5e299)
      (field "pair" tuple.const (u8.const 1) (s16.const -1))
      (field "nested" option.none)
      (field "marks" flags.const "f0" "f31"))
    (field "text" str.const "語本日")
    (field "bytes" list.const)
    (field "tag" u32.const 0)
    (field "step" s8.const -3)))

;; The innermost arm of the nested option, and every flag.
(assert_return
  (invoke "mix"
    (record.const
      (field "letter" char.const "a")
      (field "flag" bool.const true)
      (field "short" u16.const 7)
      (field "big" u64.const 1)
      (field "signed" s64.const -1)
      (field "single" f32.const 0)
      (field "double" f64.const 0.5)
      (field "pair" tuple.const (u8.const 7) (s16.const -7))
      (field "nested" option.some (option.some (u32.const 41)))
      (field "marks" flags.const "f0" "f1" "f2" "f3" "f4" "f5" "f6" "f7" "f8"
        "f9" "f10" "f11" "f12" "f13" "f14" "f15" "f16" "f17" "f18" "f19" "f20"
        "f21" "f22" "f23" "f24" "f25" "f26" "f27" "f28" "f29" "f30" "f31"))
    (str.const "")
    (list.const (u8.const 1) (u8.const 2) (u8.const 3))
    (u32.const 0)
    (s8.const 0))
  (record.const
    (field "values" record.const
      (field "letter" char.const "b")
      (field "flag" bool.const false)
      (field "short" u16.const 8)
      (field "big" u64.const 2)
      (field "signed" s64.const 1)
      (field "single" f32.const 0)
      (field "double" f64.const 0.125)
      (field "pair" tuple.const (u8.const 8) (s16.const 7))
      (field "nested" option.some (option.some (u32.const 42)))
      (field "marks" flags.const "f1" "f2" "f3" "f4" "f5" "f6" "f7" "f8" "f9"
        "f10" "f11" "f12" "f13" "f14" "f15" "f16" "f17" "f18" "f19" "f20" "f21"
        "f22" "f23" "f24" "f25" "f26" "f27" "f28" "f29" "f30"))
    (field "text" str.const "")
    (field "bytes" list.const (u8.const 254) (u8.const 253) (u8.const 252))
    (field "tag" u32.const 1)
    (field "step" s8.const 0)))

;; Negative zero and infinity in each float, and an `s64` past 2^53
;; going out and coming back, which an `f64` cannot hold exactly. The
;; runner compares floats bit for bit, so a lost sign fails.
(assert_return
  (invoke "mix"
    (record.const
      (field "letter" char.const "a")
      (field "flag" bool.const true)
      (field "short" u16.const 7)
      (field "big" u64.const 1)
      (field "signed" s64.const -9007199254740993)
      (field "single" f32.const -0.0)
      (field "double" f64.const inf)
      (field "pair" tuple.const (u8.const 7) (s16.const -7))
      (field "nested" option.none)
      (field "marks" flags.const))
    (str.const "")
    (list.const)
    (u32.const 0)
    (s8.const 0))
  (record.const
    (field "values" record.const
      (field "letter" char.const "b")
      (field "flag" bool.const false)
      (field "short" u16.const 8)
      (field "big" u64.const 2)
      (field "signed" s64.const 9007199254740993)
      (field "single" f32.const -0.0)
      (field "double" f64.const inf)
      (field "pair" tuple.const (u8.const 8) (s16.const 7))
      (field "nested" option.some (option.none))
      (field "marks" flags.const "f0" "f31"))
    (field "text" str.const "")
    (field "bytes" list.const)
    (field "tag" u32.const 1)
    (field "step" s8.const 0)))

;; The other infinity and zero, the other way round.
(assert_return
  (invoke "mix"
    (record.const
      (field "letter" char.const "a")
      (field "flag" bool.const true)
      (field "short" u16.const 7)
      (field "big" u64.const 1)
      (field "signed" s64.const 9007199254740993)
      (field "single" f32.const -inf)
      (field "double" f64.const -0.0)
      (field "pair" tuple.const (u8.const 7) (s16.const -7))
      (field "nested" option.none)
      (field "marks" flags.const))
    (str.const "")
    (list.const)
    (u32.const 0)
    (s8.const 0))
  (record.const
    (field "values" record.const
      (field "letter" char.const "b")
      (field "flag" bool.const false)
      (field "short" u16.const 8)
      (field "big" u64.const 2)
      (field "signed" s64.const -9007199254740993)
      (field "single" f32.const -inf)
      (field "double" f64.const -0.0)
      (field "pair" tuple.const (u8.const 8) (s16.const 7))
      (field "nested" option.some (option.none))
      (field "marks" flags.const "f0" "f31"))
    (field "text" str.const "")
    (field "bytes" list.const)
    (field "tag" u32.const 1)
    (field "step" s8.const 0)))

;; A NaN with a payload in each float. The specification makes every
;; lifted NaN canonical (`canonicalize_nan32` and `canonicalize_nan64` in
;; `definitions.py`), so what comes back is the canonical NaN whatever
;; payload the guest's arithmetic kept. Wasmtime keeps the payload.
(assert_return
  (invoke "mix"
    (record.const
      (field "letter" char.const "a")
      (field "flag" bool.const true)
      (field "short" u16.const 7)
      (field "big" u64.const 1)
      (field "signed" s64.const 0)
      (field "single" f32.const nan:0x200000)
      (field "double" f64.const nan:0x4000000000000)
      (field "pair" tuple.const (u8.const 7) (s16.const -7))
      (field "nested" option.none)
      (field "marks" flags.const))
    (str.const "")
    (list.const)
    (u32.const 0)
    (s8.const 0))
  (record.const
    (field "values" record.const
      (field "letter" char.const "b")
      (field "flag" bool.const false)
      (field "short" u16.const 8)
      (field "big" u64.const 2)
      (field "signed" s64.const 0)
      (field "single" f32.const nan)
      (field "double" f64.const nan)
      (field "pair" tuple.const (u8.const 8) (s16.const 7))
      (field "nested" option.some (option.none))
      (field "marks" flags.const "f0" "f31"))
    (field "text" str.const "")
    (field "bytes" list.const)
    (field "tag" u32.const 1)
    (field "step" s8.const 0)))
