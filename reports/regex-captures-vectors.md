# Regex capture/flag/replacement vectors

Go `ebe2feefc400f813aefdc8a036e66d2619da6649`. Expectations are copied from the pinned Go/RE2 tables.

- captures: 86 rows
- re2: 1636 rows
- replacement: 87 rows
- invalid: 9 rows
- Corpus programs: 245

Every excluded row:

| Source | Reason |
| --- | --- |
| find_test.go:114 | POSIX class |
| find_test.go:116 | POSIX class |
| find_test.go:129 | invalid UTF-8 (Hirsel String is valid UTF-8) |
| find_test.go:130 | invalid UTF-8 (Hirsel String is valid UTF-8) |
| find_test.go:131 | invalid UTF-8 (Hirsel String is valid UTF-8) |
| find_test.go:132 | invalid UTF-8 (Hirsel String is valid UTF-8) |
| find_test.go:133 | invalid UTF-8 (Hirsel String is valid UTF-8) |
| find_test.go:134 | invalid UTF-8 (Hirsel String is valid UTF-8) |
| re2-search.txt:2314 | RE2 byte-interior boundary; skipped by Go exec_test.go:153-159 too |
| re2-search.txt:2317 | RE2 byte-interior boundary; skipped by Go exec_test.go:153-159 too |
| re2-search.txt:2320 | RE2 byte-interior boundary; skipped by Go exec_test.go:153-159 too |
| re2-search.txt:2323 | RE2 byte-interior boundary; skipped by Go exec_test.go:153-159 too |
| re2-search.txt:2617 | unsupported escape \C |
| re2-search.txt:2618 | unsupported escape \C |
| re2-search.txt:2620 | unsupported escape \C |
| re2-search.txt:2621 | unsupported escape \C |
| re2-search.txt:2623 | unsupported escape \C |
| re2-search.txt:2624 | unsupported escape \C |
| re2-search.txt:2626 | unsupported escape \C |
| re2-search.txt:2627 | unsupported escape \C |
| re2-search.txt:2633 | unsupported escape \C |
| re2-search.txt:2634 | unsupported escape \C |
| re2-search.txt:2636 | unsupported escape \C |
| re2-search.txt:2637 | unsupported escape \C |
| re2-search.txt:2639 | unsupported escape \C |
| re2-search.txt:2640 | unsupported escape \C |
| re2-search.txt:2642 | unsupported escape \C |
| re2-search.txt:2643 | unsupported escape \C |
| re2-search.txt:2649 | unsupported escape \C |
| re2-search.txt:2650 | unsupported escape \C |
| re2-search.txt:2652 | unsupported escape \C |
| re2-search.txt:2653 | unsupported escape \C |
| re2-search.txt:2655 | unsupported escape \C |
| re2-search.txt:2656 | unsupported escape \C |
| re2-search.txt:2658 | unsupported escape \C |
| re2-search.txt:2659 | unsupported escape \C |
| re2-search.txt:2761 | unsupported escape \1 |
| re2-search.txt:2762 | unsupported escape \1 |
| re2-search.txt:2764 | unsupported escape \1 |
| re2-search.txt:2765 | unsupported escape \1 |
| re2-search.txt:2767 | unsupported escape \1 |
| re2-search.txt:2768 | unsupported escape \1 |
| re2-search.txt:2770 | unsupported escape \1 |
| re2-search.txt:2771 | unsupported escape \1 |
| re2-search.txt:2777 | unsupported escape \0 |
| re2-search.txt:2778 | unsupported escape \0 |
| re2-search.txt:2780 | unsupported escape \0 |
| re2-search.txt:2781 | unsupported escape \0 |
| re2-search.txt:2783 | unsupported escape \0 |
| re2-search.txt:2784 | unsupported escape \0 |
| re2-search.txt:2786 | unsupported escape \0 |
| re2-search.txt:2787 | unsupported escape \0 |
| re2-search.txt:2793 | unsupported escape \0 |
| re2-search.txt:2794 | unsupported escape \0 |
| re2-search.txt:2796 | unsupported escape \0 |
| re2-search.txt:2797 | unsupported escape \0 |
| re2-search.txt:2799 | unsupported escape \0 |
| re2-search.txt:2800 | unsupported escape \0 |
| re2-search.txt:2802 | unsupported escape \0 |
| re2-search.txt:2803 | unsupported escape \0 |
| re2-search.txt:2809 | unsupported escape \6 |
| re2-search.txt:2810 | unsupported escape \6 |
| re2-search.txt:2812 | unsupported escape \6 |
| re2-search.txt:2813 | unsupported escape \6 |
| re2-search.txt:2815 | unsupported escape \6 |
| re2-search.txt:2816 | unsupported escape \6 |
| re2-search.txt:2818 | unsupported escape \6 |
| re2-search.txt:2819 | unsupported escape \6 |
| re2-search.txt:2825 | unsupported escape \0 |
| re2-search.txt:2826 | unsupported escape \0 |
| re2-search.txt:2828 | unsupported escape \0 |
| re2-search.txt:2829 | unsupported escape \0 |
| re2-search.txt:2831 | unsupported escape \0 |
| re2-search.txt:2832 | unsupported escape \0 |
| re2-search.txt:2834 | unsupported escape \0 |
| re2-search.txt:2835 | unsupported escape \0 |
| re2-search.txt:2841 | unsupported escape \0 |
| re2-search.txt:2842 | unsupported escape \0 |
| re2-search.txt:2844 | unsupported escape \0 |
| re2-search.txt:2845 | unsupported escape \0 |
| re2-search.txt:2847 | unsupported escape \0 |
| re2-search.txt:2848 | unsupported escape \0 |
| re2-search.txt:2850 | unsupported escape \0 |
| re2-search.txt:2851 | unsupported escape \0 |
| re2-search.txt:2857 | unsupported escape \x |
| re2-search.txt:2858 | unsupported escape \x |
| re2-search.txt:2860 | unsupported escape \x |
| re2-search.txt:2861 | unsupported escape \x |
| re2-search.txt:2863 | unsupported escape \x |
| re2-search.txt:2864 | unsupported escape \x |
| re2-search.txt:2866 | unsupported escape \x |
| re2-search.txt:2867 | unsupported escape \x |
| re2-search.txt:2873 | unsupported escape \x |
| re2-search.txt:2874 | unsupported escape \x |
| re2-search.txt:2876 | unsupported escape \x |
| re2-search.txt:2877 | unsupported escape \x |
| re2-search.txt:2879 | unsupported escape \x |
| re2-search.txt:2880 | unsupported escape \x |
| re2-search.txt:2882 | unsupported escape \x |
| re2-search.txt:2883 | unsupported escape \x |
| re2-search.txt:2889 | unsupported escape \x |
| re2-search.txt:2890 | unsupported escape \x |
| re2-search.txt:2892 | unsupported escape \x |
| re2-search.txt:2893 | unsupported escape \x |
| re2-search.txt:2895 | unsupported escape \x |
| re2-search.txt:2896 | unsupported escape \x |
| re2-search.txt:2898 | unsupported escape \x |
| re2-search.txt:2899 | unsupported escape \x |
| re2-search.txt:2905 | unsupported escape \p |
| re2-search.txt:2906 | unsupported escape \p |
| re2-search.txt:2908 | unsupported escape \p |
| re2-search.txt:2909 | unsupported escape \p |
| re2-search.txt:2911 | unsupported escape \p |
| re2-search.txt:2912 | unsupported escape \p |
| re2-search.txt:2914 | unsupported escape \p |
| re2-search.txt:2915 | unsupported escape \p |
| re2-search.txt:2921 | unsupported escape \P |
| re2-search.txt:2922 | unsupported escape \P |
| re2-search.txt:2924 | unsupported escape \P |
| re2-search.txt:2925 | unsupported escape \P |
| re2-search.txt:2927 | unsupported escape \P |
| re2-search.txt:2928 | unsupported escape \P |
| re2-search.txt:2930 | unsupported escape \P |
| re2-search.txt:2931 | unsupported escape \P |
| re2-search.txt:2937 | unsupported escape \p |
| re2-search.txt:2938 | unsupported escape \p |
| re2-search.txt:2940 | unsupported escape \p |
| re2-search.txt:2941 | unsupported escape \p |
| re2-search.txt:2943 | unsupported escape \p |
| re2-search.txt:2944 | unsupported escape \p |
| re2-search.txt:2946 | unsupported escape \p |
| re2-search.txt:2947 | unsupported escape \p |
| re2-search.txt:2953 | unsupported escape \P |
| re2-search.txt:2954 | unsupported escape \P |
| re2-search.txt:2956 | unsupported escape \P |
| re2-search.txt:2957 | unsupported escape \P |
| re2-search.txt:2959 | unsupported escape \P |
| re2-search.txt:2960 | unsupported escape \P |
| re2-search.txt:2962 | unsupported escape \P |
| re2-search.txt:2963 | unsupported escape \P |
| re2-search.txt:2985 | unsupported escape \p |
| re2-search.txt:2986 | unsupported escape \p |
| re2-search.txt:2988 | unsupported escape \p |
| re2-search.txt:2989 | unsupported escape \p |
| re2-search.txt:2991 | unsupported escape \p |
| re2-search.txt:2992 | unsupported escape \p |
| re2-search.txt:2994 | unsupported escape \p |
| re2-search.txt:2995 | unsupported escape \p |
| re2-search.txt:3001 | unsupported escape \p |
| re2-search.txt:3002 | unsupported escape \p |
| re2-search.txt:3004 | unsupported escape \p |
| re2-search.txt:3005 | unsupported escape \p |
| re2-search.txt:3007 | unsupported escape \p |
| re2-search.txt:3008 | unsupported escape \p |
| re2-search.txt:3010 | unsupported escape \p |
| re2-search.txt:3011 | unsupported escape \p |
| re2-search.txt:3017 | unsupported escape \P |
| re2-search.txt:3018 | unsupported escape \P |
| re2-search.txt:3020 | unsupported escape \P |
| re2-search.txt:3021 | unsupported escape \P |
| re2-search.txt:3023 | unsupported escape \P |
| re2-search.txt:3024 | unsupported escape \P |
| re2-search.txt:3026 | unsupported escape \P |
| re2-search.txt:3027 | unsupported escape \P |
| re2-search.txt:3033 | unsupported escape \P |
| re2-search.txt:3034 | unsupported escape \P |
| re2-search.txt:3036 | unsupported escape \P |
| re2-search.txt:3037 | unsupported escape \P |
| re2-search.txt:3039 | unsupported escape \P |
| re2-search.txt:3040 | unsupported escape \P |
| re2-search.txt:3042 | unsupported escape \P |
| re2-search.txt:3043 | unsupported escape \P |
| re2-search.txt:3049 | unsupported escape \p |
| re2-search.txt:3050 | unsupported escape \p |
| re2-search.txt:3052 | unsupported escape \p |
| re2-search.txt:3053 | unsupported escape \p |
| re2-search.txt:3055 | unsupported escape \p |
| re2-search.txt:3056 | unsupported escape \p |
| re2-search.txt:3058 | unsupported escape \p |
| re2-search.txt:3059 | unsupported escape \p |
| re2-search.txt:3065 | unsupported escape \p |
| re2-search.txt:3066 | unsupported escape \p |
| re2-search.txt:3068 | unsupported escape \p |
| re2-search.txt:3069 | unsupported escape \p |
| re2-search.txt:3071 | unsupported escape \p |
| re2-search.txt:3072 | unsupported escape \p |
| re2-search.txt:3074 | unsupported escape \p |
| re2-search.txt:3075 | unsupported escape \p |
| re2-search.txt:3081 | unsupported escape \p |
| re2-search.txt:3082 | unsupported escape \p |
| re2-search.txt:3084 | unsupported escape \p |
| re2-search.txt:3085 | unsupported escape \p |
| re2-search.txt:3087 | unsupported escape \p |
| re2-search.txt:3088 | unsupported escape \p |
| re2-search.txt:3090 | unsupported escape \p |
| re2-search.txt:3091 | unsupported escape \p |
| re2-search.txt:3097 | unsupported escape \p |
| re2-search.txt:3098 | unsupported escape \p |
| re2-search.txt:3100 | unsupported escape \p |
| re2-search.txt:3101 | unsupported escape \p |
| re2-search.txt:3103 | unsupported escape \p |
| re2-search.txt:3104 | unsupported escape \p |
| re2-search.txt:3106 | unsupported escape \p |
| re2-search.txt:3107 | unsupported escape \p |
| re2-search.txt:3625 | unsupported escape \C |
| re2-search.txt:3626 | unsupported escape \C |
| re2-search.txt:3628 | unsupported escape \C |
| re2-search.txt:3629 | unsupported escape \C |
| re2-search.txt:3631 | unsupported escape \C |
| re2-search.txt:3632 | unsupported escape \C |
| re2-search.txt:3634 | unsupported escape \C |
| re2-search.txt:3635 | unsupported escape \C |
| re2-search.txt:3641 | unsupported escape \C |
| re2-search.txt:3642 | unsupported escape \C |
| re2-search.txt:3644 | unsupported escape \C |
| re2-search.txt:3645 | unsupported escape \C |
| re2-search.txt:3647 | unsupported escape \C |
| re2-search.txt:3648 | unsupported escape \C |
| re2-search.txt:3650 | unsupported escape \C |
| re2-search.txt:3651 | unsupported escape \C |
| re2-search.txt:3657 | unsupported escape \C |
| re2-search.txt:3658 | unsupported escape \C |
| re2-search.txt:3660 | unsupported escape \C |
| re2-search.txt:3661 | unsupported escape \C |
| re2-search.txt:3663 | unsupported escape \C |
| re2-search.txt:3664 | unsupported escape \C |
| re2-search.txt:3666 | unsupported escape \C |
| re2-search.txt:3667 | unsupported escape \C |
| re2-search.txt:3673 | unsupported escape \C |
| re2-search.txt:3674 | unsupported escape \C |
| re2-search.txt:3676 | unsupported escape \C |
| re2-search.txt:3677 | unsupported escape \C |
| re2-search.txt:3679 | unsupported escape \C |
| re2-search.txt:3680 | unsupported escape \C |
| re2-search.txt:3682 | unsupported escape \C |
| re2-search.txt:3683 | unsupported escape \C |
| re2-search.txt:3689 | unsupported escape \C |
| re2-search.txt:3690 | unsupported escape \C |
| re2-search.txt:3692 | unsupported escape \C |
| re2-search.txt:3693 | unsupported escape \C |
| re2-search.txt:3695 | unsupported escape \C |
| re2-search.txt:3696 | unsupported escape \C |
| re2-search.txt:3698 | unsupported escape \C |
| re2-search.txt:3699 | unsupported escape \C |
| re2-search.txt:3705 | unsupported escape \C |
| re2-search.txt:3706 | unsupported escape \C |
| re2-search.txt:3708 | unsupported escape \C |
| re2-search.txt:3709 | unsupported escape \C |
| re2-search.txt:3711 | unsupported escape \C |
| re2-search.txt:3712 | unsupported escape \C |
| re2-search.txt:3714 | unsupported escape \C |
| re2-search.txt:3715 | unsupported escape \C |
| re2-search.txt:3721 | unsupported escape \C |
| re2-search.txt:3722 | unsupported escape \C |
| re2-search.txt:3724 | unsupported escape \C |
| re2-search.txt:3725 | unsupported escape \C |
| re2-search.txt:3727 | unsupported escape \C |
| re2-search.txt:3728 | unsupported escape \C |
| re2-search.txt:3730 | unsupported escape \C |
| re2-search.txt:3731 | unsupported escape \C |
| syntax/parse_test.go:490 | flags, named groups or lookaround |
| syntax/parse_test.go:495 | flags, named groups or lookaround |
